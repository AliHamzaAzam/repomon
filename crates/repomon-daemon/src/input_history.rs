//! `agent.input_history`: the agent's own up-arrow recall.
//!
//! The chat composer and the agent's TUI should walk one history, not two. They already share a
//! source without any merging: a chat send reaches the CLI as keystrokes and the CLI writes it
//! into its own history exactly as if it had been typed there, so reading that one store unifies
//! both directions. This module never combines two lists.
//!
//! Resolution order is the window's bound session, then the lane's working directory, then
//! nothing. A window never receives another window's session entries.

use crate::Ctx;
use chrono::{DateTime, Utc};
use repomon_core::model::LaneId;
use std::path::{Path, PathBuf};

/// One recalled submission.
pub struct Entry {
    pub text: String,
    pub at: Option<DateTime<Utc>>,
}

/// Which scope produced the entries, so the client can say why recall is empty.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// The window's own session id matched entries.
    Session,
    /// The session had none yet, so the lane's working directory supplied them.
    Project,
    /// No readable history for this kind, or nothing in either scope.
    None,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Scope::Session => "session",
            Scope::Project => "project",
            Scope::None => "none",
        }
    }
}

/// Newest `limit` entries after collapsing runs, so a client walking backwards never has to skip
/// repeats of the same submission.
const DEFAULT_LIMIT: usize = 200;

fn claude_history_paths() -> Vec<PathBuf> {
    if let Ok(root) = std::env::var("REPOMON_CLAUDE_HISTORY") {
        return vec![PathBuf::from(root)];
    }
    repomon_core::agent::claude::config_bases()
        .into_iter()
        .map(|base| base.join("history.jsonl"))
        .collect()
}

fn codex_history_path() -> PathBuf {
    std::env::var_os("REPOMON_CODEX_HISTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".codex/history.jsonl"))
                .unwrap_or_else(|| PathBuf::from(".codex/history.jsonl"))
        })
}

/// Both CLIs record their timestamp as a string of digits; Claude's are milliseconds and Codex's
/// are seconds. Accept a JSON number too, so a format change to a real number still parses.
fn stamp(value: &serde_json::Value, millis: bool) -> Option<DateTime<Utc>> {
    let raw = value
        .as_str()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .or_else(|| value.as_i64())?;
    if millis {
        DateTime::from_timestamp_millis(raw)
    } else {
        DateTime::from_timestamp(raw, 0)
    }
}

fn read_jsonl(path: &Path) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Collapse consecutive duplicates, then keep the newest `limit`, oldest first.
fn finish(mut entries: Vec<Entry>, limit: usize) -> Vec<Entry> {
    entries.retain(|e| !e.text.trim().is_empty());
    entries.dedup_by(|a, b| a.text == b.text);
    let extra = entries.len().saturating_sub(limit);
    entries.drain(..extra);
    entries
}

/// Claude Code's own recall file, which carries both a session id and the project directory, so
/// each scope is a direct filter.
fn claude(session: Option<&str>, cwd: &Path, limit: usize) -> (Vec<Entry>, Scope) {
    let rows: Vec<_> = claude_history_paths()
        .iter()
        .flat_map(|p| read_jsonl(p))
        .collect();
    let collect = |keep: &dyn Fn(&serde_json::Value) -> bool| -> Vec<Entry> {
        rows.iter()
            .filter(|row| keep(row))
            .filter_map(|row| {
                Some(Entry {
                    text: row.get("display")?.as_str()?.to_string(),
                    at: row.get("timestamp").and_then(|v| stamp(v, true)),
                })
            })
            .collect()
    };
    if let Some(session) = session {
        let found = finish(
            collect(&|row| row.get("sessionId").and_then(|v| v.as_str()) == Some(session)),
            limit,
        );
        if !found.is_empty() {
            return (found, Scope::Session);
        }
    }
    let wanted = cwd.to_string_lossy().to_string();
    let found = finish(
        collect(&|row| row.get("project").and_then(|v| v.as_str()) == Some(wanted.as_str())),
        limit,
    );
    let scope = if found.is_empty() {
        Scope::None
    } else {
        Scope::Project
    };
    (found, scope)
}

/// Codex's recall file records a session id but no working directory, so the project scope is the
/// set of its rollouts that recorded this cwd. Bounded to recent rollouts: this only runs when the
/// window's own session has no entries yet.
fn codex_sessions_for(cwd: &Path) -> Vec<String> {
    fn entries(path: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect()
    }
    let root = std::env::var_os("REPOMON_CODEX_SESSIONS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".codex/sessions"))
                .unwrap_or_default()
        });
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for year in entries(&root) {
        for month in entries(&year) {
            for day in entries(&month) {
                for path in entries(&day) {
                    if path.extension().is_none_or(|e| e != "jsonl") {
                        continue;
                    }
                    if let Ok(at) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                        files.push((at, path));
                    }
                }
            }
        }
    }
    files.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    files.truncate(200);
    files
        .into_iter()
        .filter_map(|(_, path)| {
            use std::io::{BufRead, BufReader, Read};
            let file = std::fs::File::open(&path).ok()?;
            let mut line = String::new();
            BufReader::new(file.take(128 * 1024))
                .read_line(&mut line)
                .ok()?;
            let v: serde_json::Value = serde_json::from_str(&line).ok()?;
            if v["type"] != "session_meta" {
                return None;
            }
            let recorded = v["payload"]["cwd"].as_str()?;
            (Path::new(recorded) == cwd).then(|| {
                v["payload"]["id"]
                    .as_str()
                    .or_else(|| v["payload"]["session_id"].as_str())
                    .map(str::to_string)
            })?
        })
        .collect()
}

fn codex(session: Option<&str>, cwd: &Path, limit: usize) -> (Vec<Entry>, Scope) {
    let rows = read_jsonl(&codex_history_path());
    let collect = |ids: &[String]| -> Vec<Entry> {
        rows.iter()
            .filter(|row| {
                row.get("session_id")
                    .and_then(|v| v.as_str())
                    .is_some_and(|id| ids.iter().any(|want| want == id))
            })
            .filter_map(|row| {
                Some(Entry {
                    text: row.get("text")?.as_str()?.to_string(),
                    at: row.get("ts").and_then(|v| stamp(v, false)),
                })
            })
            .collect()
    };
    if let Some(session) = session {
        let found = finish(collect(&[session.to_string()]), limit);
        if !found.is_empty() {
            return (found, Scope::Session);
        }
    }
    let found = finish(collect(&codex_sessions_for(cwd)), limit);
    let scope = if found.is_empty() {
        Scope::None
    } else {
        Scope::Project
    };
    (found, scope)
}

/// The stores with no separate recall file: their user messages are the equivalent, read through
/// the same scanners the conversation view uses.
fn from_scanner(
    kind: &str,
    session: Option<&str>,
    cwd: &Path,
    limit: usize,
) -> (Vec<Entry>, Scope) {
    use repomon_core::usage_ledger::scan::ScanOptions;
    let options = || ScanOptions {
        collect_transcript: true,
        before_offset: None,
    };
    let rows = |session: &str| -> Vec<Entry> {
        let scan = match kind {
            "opencode" => repomon_core::usage_ledger::scan::scan_opencode_db_with_options(
                &repomon_core::agent::opencode::database_path(),
                0,
                Some(session),
                options(),
            ),
            "hermes" => repomon_core::usage_ledger::hermes::scan(
                &repomon_core::usage_ledger::hermes::database_path(),
                0,
                session,
                options(),
            ),
            "antigravity" => {
                let path = repomon_core::agent::antigravity::cache_path()
                    .parent()
                    .and_then(Path::parent)
                    .unwrap_or(Path::new("."))
                    .join("brain")
                    .join(session)
                    .join(".system_generated/logs/transcript.jsonl");
                repomon_core::usage_ledger::scan::scan_antigravity_transcript_with_options(
                    &path,
                    0,
                    "gemini-3",
                    None,
                    options(),
                )
            }
            _ => return Vec::new(),
        };
        scan.map(|scan| {
            scan.transcript
                .into_iter()
                .filter(|r| r.item.kind.as_deref() == Some("user"))
                .map(|r| Entry {
                    text: repomon_core::usage_ledger::scan::strip_injected_blocks(&r.item.text)
                        .trim()
                        .to_string(),
                    at: r.item.at,
                })
                .collect()
        })
        .unwrap_or_default()
    };
    if let Some(session) = session {
        let found = finish(rows(session), limit);
        if !found.is_empty() {
            return (found, Scope::Session);
        }
    }
    // Project scope: every session this kind records against the lane's directory.
    let peers: Vec<String> = match kind {
        "opencode" => repomon_core::agent::opencode::session_ids_since(cwd, DateTime::UNIX_EPOCH),
        "hermes" => repomon_core::usage_ledger::hermes::sessions(
            &repomon_core::usage_ledger::hermes::database_path(),
        )
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.cwd.as_deref().is_some_and(|p| Path::new(p) == cwd))
        .map(|s| s.session_id)
        .collect(),
        _ => Vec::new(),
    };
    let mut all: Vec<Entry> = peers.iter().flat_map(|id| rows(id)).collect();
    all.sort_by_key(|e| e.at);
    let found = finish(all, limit);
    let scope = if found.is_empty() {
        Scope::None
    } else {
        Scope::Project
    };
    (found, scope)
}

/// Resolve one window's recall history. `window` must belong to `lane`; the caller enforces that
/// the same way the transcript RPCs do.
pub async fn history(
    ctx: &Ctx,
    lane: LaneId,
    window: String,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, 10_000);
    let cwd = ctx.lanes.focus(lane).await.map_err(|e| e.to_string())?;
    let backend = ctx.backend.clone();
    let target = window.clone();
    let meta = tokio::task::spawn_blocking(move || {
        backend
            .list_windows_meta()
            .ok()
            .and_then(|all| all.into_iter().find(|w| w.name == target))
    })
    .await
    .map_err(|e| e.to_string())?;
    let kind = meta
        .as_ref()
        .and_then(|m| m.agent_kind.clone())
        .unwrap_or_else(|| "claude-code".into());
    let session = meta.as_ref().and_then(|m| m.session.clone());
    let (entries, scope) = tokio::task::spawn_blocking(move || match kind.as_str() {
        "claude-code" => claude(session.as_deref(), &cwd, limit),
        "codex" => codex(session.as_deref(), &cwd, limit),
        "opencode" | "hermes" | "antigravity" => {
            from_scanner(&kind, session.as_deref(), &cwd, limit)
        }
        _ => (Vec::new(), Scope::None),
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "entries": entries
            .into_iter()
            .map(|e| serde_json::json!({ "text": e.text, "at": e.at }))
            .collect::<Vec<_>>(),
        "source": scope.as_str(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Provider path overrides are process global, so history tests serialize on this.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<T>(vars: &[(&str, &Path)], f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            // SAFETY: single threaded within the lock.
            unsafe { std::env::set_var(k, v) };
        }
        let out = f();
        for (k, _) in vars {
            // SAFETY: as above.
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    #[test]
    fn claude_recall_prefers_the_window_session_then_falls_back_to_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("history.jsonl");
        // The real shape: display, pastedContents, a millisecond timestamp as a string, project,
        // sessionId.
        let rows = [
            r#"{"display":"older project line","pastedContents":"{}","timestamp":"1771269516231","project":"/repo","sessionId":"other"}"#,
            r#"{"display":"mine one","pastedContents":"{}","timestamp":"1771269516232","project":"/repo","sessionId":"mine"}"#,
            r#"{"display":"mine one","pastedContents":"{}","timestamp":"1771269516233","project":"/repo","sessionId":"mine"}"#,
            r#"{"display":"mine two","pastedContents":"{}","timestamp":"1771269516234","project":"/repo","sessionId":"mine"}"#,
            r#"{"display":"elsewhere","pastedContents":"{}","timestamp":"1771269516235","project":"/other","sessionId":"far"}"#,
        ];
        std::fs::write(&file, rows.join("\n")).unwrap();
        with_env(&[("REPOMON_CLAUDE_HISTORY", &file)], || {
            let (entries, scope) = claude(Some("mine"), Path::new("/repo"), 200);
            assert_eq!(scope, Scope::Session);
            // Oldest first, consecutive duplicates collapsed, no other session's lines.
            assert_eq!(
                entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
                ["mine one", "mine two"]
            );
            assert!(entries[0].at.is_some(), "millisecond stamps must parse");

            // A window whose session has no entries yet falls back to the directory, and that
            // scope legitimately spans the other sessions in it, but never another directory.
            let (entries, scope) = claude(Some("fresh"), Path::new("/repo"), 200);
            assert_eq!(scope, Scope::Project);
            let texts: Vec<_> = entries.iter().map(|e| e.text.as_str()).collect();
            assert!(texts.contains(&"older project line") && texts.contains(&"mine two"));
            assert!(!texts.contains(&"elsewhere"));

            // Nothing anywhere is "none", not an empty "project".
            let (entries, scope) = claude(Some("fresh"), Path::new("/nowhere"), 200);
            assert!(entries.is_empty());
            assert_eq!(scope, Scope::None);

            // `limit` caps the newest N, after collapsing.
            let (entries, _) = claude(Some("fresh"), Path::new("/repo"), 2);
            assert_eq!(entries.len(), 2);
            assert_eq!(entries.last().unwrap().text, "mine two");
        });
    }

    #[test]
    fn codex_recall_uses_its_session_id_and_seconds_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("history.jsonl");
        // The real shape: session_id, ts in seconds as a string, text.
        let rows = [
            r#"{"session_id":"mine","ts":"1781823706","text":"hi"}"#,
            r#"{"session_id":"mine","ts":"1781823707","text":"hi"}"#,
            r#"{"session_id":"mine","ts":"1781823708","text":"write a poem"}"#,
            r#"{"session_id":"other","ts":"1781823709","text":"not mine"}"#,
        ];
        std::fs::write(&file, rows.join("\n")).unwrap();
        let sessions = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        with_env(
            &[
                ("REPOMON_CODEX_HISTORY", &file),
                ("REPOMON_CODEX_SESSIONS", &sessions),
            ],
            || {
                let (entries, scope) = codex(Some("mine"), Path::new("/repo"), 200);
                assert_eq!(scope, Scope::Session);
                assert_eq!(
                    entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
                    ["hi", "write a poem"]
                );
                assert_eq!(
                    entries[0].at.map(|t| t.timestamp()),
                    Some(1781823706),
                    "second stamps must parse as seconds, not milliseconds"
                );
                // No rollout records this directory, so a fresh session recalls nothing rather
                // than borrowing another session's lines.
                let (entries, scope) = codex(Some("fresh"), Path::new("/repo"), 200);
                assert!(entries.is_empty());
                assert_eq!(scope, Scope::None);
            },
        );
    }

    #[test]
    fn hermes_recall_reads_its_sqlite_user_rows_for_the_bound_session_only() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.db");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(include_str!(
            "../../repomon-core/src/usage_ledger/fixtures/hermes_conversation_v0.sql"
        ))
        .unwrap();
        drop(conn);
        with_env(&[("REPOMON_HERMES_DB", &db)], || {
            let (entries, scope) =
                from_scanner("hermes", Some("hermes-fixture"), Path::new("/x"), 200);
            assert_eq!(scope, Scope::Session);
            assert!(
                entries
                    .iter()
                    .any(|e| e.text.contains("inspect the Hermes fixture repository")),
                "{entries:?}",
                entries = entries.iter().map(|e| &e.text).collect::<Vec<_>>()
            );
            assert!(
                !entries.iter().any(|e| e.text.contains("Unrelated session")),
                "another session's prompt leaked into recall"
            );
            // Every Hermes session on this machine records a null cwd, so a fresh session has no
            // project scope to fall back to and says so rather than guessing.
            let (entries, scope) = from_scanner("hermes", Some("fresh"), Path::new("/x"), 200);
            assert!(entries.is_empty());
            assert_eq!(scope, Scope::None);
        });
    }

    #[test]
    fn an_unknown_kind_reports_none_rather_than_an_empty_session() {
        let (entries, scope) = from_scanner("cursor", Some("s"), Path::new("/x"), 200);
        assert!(entries.is_empty());
        assert_eq!(scope, Scope::None);
    }
}
