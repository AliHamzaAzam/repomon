//! Provider identities require exact session stamps or unique, recent pane evidence.
use super::*;
use chrono::{DateTime, Utc};
use repomon_core::agent::SessionBackend;
use std::path::Path;

fn entries(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect()
}
fn root_codex() -> PathBuf {
    std::env::var_os("REPOMON_CODEX_SESSIONS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            directories::BaseDirs::new()
                .unwrap()
                .home_dir()
                .join(".codex/sessions")
        })
}
fn root_agy() -> PathBuf {
    repomon_core::agent::antigravity::cache_path()
        .parent()
        .and_then(Path::parent)
        .unwrap_or(Path::new("."))
        .to_path_buf()
}
fn codex_meta(path: &Path) -> Option<(String, Option<PathBuf>)> {
    use std::io::{BufRead, BufReader, Read};
    let file = std::fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file.take(128 * 1024))
        .read_line(&mut line)
        .ok()?;
    let v: Value = serde_json::from_str(&line).ok()?;
    if v["type"] != "session_meta" {
        return None;
    }
    let p = &v["payload"];
    Some((
        p["id"]
            .as_str()
            .or_else(|| p["session_id"].as_str())?
            .into(),
        p["cwd"].as_str().map(PathBuf::from),
    ))
}
fn same_path(a: &Path, b: &Path) -> bool {
    a.canonicalize().unwrap_or_else(|_| a.into()) == b.canonicalize().unwrap_or_else(|_| b.into())
}
// Antigravity may record the main checkout while the managed lane lives in a linked worktree.
// This only widens candidate discovery; it never authorizes a binding without pane evidence.
fn main_checkout(cwd: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    PathBuf::from(String::from_utf8(out.stdout).ok()?.trim())
        .parent()
        .map(Path::to_path_buf)
}
fn candidates(kind: &str, cwd: &Path, bound: Option<&str>, started: DateTime<Utc>) -> Vec<Source> {
    let recent = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .is_some_and(|at| DateTime::<Utc>::from(at) >= started)
    };
    let mut found = Vec::new();
    let mut add = |path: PathBuf, id: String| {
        if bound.is_none_or(|wanted| wanted == id) {
            found.push(Source {
                window: String::new(),
                kind: kind.into(),
                path: Some(path),
                session: Some(id),
            });
        }
    };
    match kind {
        "codex" => {
            for year in entries(&root_codex()) {
                for month in entries(&year) {
                    for day in entries(&month) {
                        for path in entries(&day) {
                            if path.extension().is_none_or(|e| e != "jsonl") {
                                continue;
                            }
                            // Session filenames contain the ID, but the session_meta row is authoritative.
                            if bound.is_some_and(|id| {
                                !path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .contains(id)
                            }) {
                                continue;
                            }
                            if !recent(&path) {
                                continue;
                            }
                            if let Some((id, recorded)) = codex_meta(&path) {
                                if bound.is_some()
                                    || recorded.as_deref().is_some_and(|p| same_path(p, cwd))
                                {
                                    add(path, id);
                                }
                            }
                        }
                    }
                }
            }
        }
        "antigravity" => {
            let root = root_agy();
            let main = main_checkout(cwd);
            let mappings: std::collections::HashMap<String, String> =
                std::fs::read(repomon_core::agent::antigravity::cache_path())
                    .ok()
                    .and_then(|v| serde_json::from_slice(&v).ok())
                    .unwrap_or_default();
            for dir in entries(&root.join("brain")) {
                let Some(id) = dir.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                if bound.is_none()
                    && mappings.iter().any(|(path, sid)| {
                        sid == id
                            && !same_path(Path::new(path), cwd)
                            && main
                                .as_deref()
                                .is_none_or(|m| !same_path(Path::new(path), m))
                    })
                {
                    continue;
                }
                let path = dir.join(".system_generated/logs/transcript.jsonl");
                if path.is_file() && recent(&path) {
                    add(path, id.into());
                }
            }
        }
        "aider" => {
            let path = cwd.join(".aider.chat.history.md");
            if let Ok(scan) =
                repomon_core::usage_ledger::aider::scan(&path, None, 0, ScanOptions::default())
            {
                for s in scan.sessions {
                    add(path.clone(), s.session_id);
                }
            }
        }
        "hermes" => {
            let path = repomon_core::usage_ledger::hermes::database_path();
            if let Ok(sessions) = repomon_core::usage_ledger::hermes::sessions(&path) {
                for s in sessions {
                    if bound.is_some()
                        || s.cwd
                            .as_deref()
                            .is_none_or(|p| same_path(Path::new(p), cwd))
                    {
                        add(path.clone(), s.session_id);
                    }
                }
            }
        }
        "opencode" => {
            let path = repomon_core::agent::opencode::database_path();
            if let Some(id) = bound {
                add(path, id.into());
            } else {
                for id in repomon_core::agent::opencode::session_ids_since(cwd, started) {
                    add(path.clone(), id);
                }
            }
        }
        _ => {}
    }
    found
}
fn normalize(s: &str) -> String {
    crate::rpc::normalize_fingerprint(s)
}
fn evidence(scan: &SourceScan, pane: &str, started: DateTime<Utc>, kind: &str) -> bool {
    let pane = normalize(pane);
    scan.transcript
        .iter()
        .rev()
        .filter(|r| {
            matches!(r.item.kind.as_deref(), Some("user" | "assistant"))
                && r.item.at.is_some_and(|at| at >= started)
        })
        .take(8)
        .any(|row| {
            crate::rpc::message_fingerprint(Some(&row.item.text))
                .is_some_and(|needle| pane.contains(&needle))
                // Sidebars interrupt wrapped prose in OpenCode's grid. A distinctive complete
                // line is still evidence, subject to the same session and peer ambiguity checks.
                || (kind == "opencode" && row.item.text.lines().any(|line| crate::rpc::message_fingerprint(Some(line))
                    .is_some_and(|needle| pane.contains(&needle))))
        })
}
/// No newest-cwd fallback. Ambiguity between sessions or between live windows stays unresolved.
pub(super) struct Request<'a> {
    pub window: &'a str,
    pub kind: &'a str,
    pub cwd: &'a Path,
    pub bound: Option<&'a str>,
    pub started: DateTime<Utc>,
}
pub(super) fn discover(
    cache: &Cache,
    backend: &dyn SessionBackend,
    request: Request<'_>,
) -> Option<Source> {
    let sources = candidates(request.kind, request.cwd, request.bound, request.started);
    select(cache, backend, request, sources)
}
fn select(
    cache: &Cache,
    backend: &dyn SessionBackend,
    request: Request<'_>,
    sources: Vec<Source>,
) -> Option<Source> {
    let Request {
        window,
        kind,
        cwd: _,
        bound,
        started,
    } = request;
    let windows = backend.list_windows_meta().ok()?;
    let peers: Vec<_> = windows
        .iter()
        .filter(|w| w.name != window && w.agent_kind.as_deref() == Some(kind))
        .collect();
    if kind == "aider" && !peers.is_empty() {
        return None;
    }
    let pane = if bound.is_none() {
        backend.capture_named(window, CaptureOpts::last(500)).ok()?
    } else {
        String::new()
    };
    let mut matches = Vec::new();
    for mut src in sources {
        if bound.is_none() && peers.iter().any(|w| w.session == src.session) {
            continue;
        }
        let Ok(parsed) = cache.get(&src, None) else {
            continue;
        };
        let latest = parsed
            .scan
            .transcript
            .iter()
            .filter_map(|r| r.item.at)
            .max()
            .or_else(|| parsed.scan.sessions.iter().filter_map(|s| s.last_at).max());
        if latest.is_none_or(|at| at < started) {
            continue;
        }
        if bound.is_none() {
            if !evidence(&parsed.scan, &pane, started, kind) {
                continue;
            }
            if peers.iter().any(|peer| {
                backend
                    .capture_named(&peer.name, CaptureOpts::last(500))
                    .ok()
                    .is_none_or(|text| evidence(&parsed.scan, &text, started, kind))
            }) {
                continue;
            }
        }
        src.window = window.into();
        matches.push(src);
    }
    if matches.len() != 1 {
        return None;
    }
    let src = matches.pop()?;
    // Revalidate the immutable window ID and age after scanning and before stamping.
    let original = windows.iter().find(|w| w.name == window)?;
    if backend.window_started_at(window) != Some(started) {
        return None;
    }
    let current = backend
        .list_windows_meta()
        .ok()?
        .into_iter()
        .find(|w| w.wid == original.wid && w.name == window)?;
    if current.session.as_deref() != bound {
        return None;
    }
    if bound.is_none() {
        backend
            .set_window_session_by_id(original.wid, src.session.as_deref()?)
            .ok()?;
    }
    Some(src)
}

#[cfg(test)]
mod tests {
    use super::super::tests::ScriptedBackend;
    use super::*;
    use repomon_core::agent::WindowMeta;
    fn fixture(dir: &Path, kind: &str, id: &str, text: &str) -> Source {
        let at = "2026-09-11T12:00:00Z";
        let path = if kind == "antigravity" {
            dir.join(id).join(".system_generated/logs/transcript.jsonl")
        } else {
            dir.join(format!("{id}.jsonl"))
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let row = match kind {
            "claude-code" => json!({"type":"assistant","timestamp":at,"message":{"content":text}}),
            "codex" => {
                json!({"type":"response_item","timestamp":at,"payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":text}]}})
            }
            _ => {
                json!({"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":at,"content":text})
            }
        };
        let meta = if kind == "codex" {
            format!(
                "{}\n",
                json!({"type":"session_meta","timestamp":at,"payload":{"id":id,"cwd":dir}})
            )
        } else {
            String::new()
        };
        std::fs::write(&path, format!("{meta}{row}\n")).unwrap();
        Source {
            window: String::new(),
            kind: kind.into(),
            path: Some(path),
            session: Some(id.into()),
        }
    }
    #[test]
    fn database_sources_bind_from_distinctive_lines_even_with_sidebar_columns() {
        for kind in ["opencode", "hermes"] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("state.db");
            let conn = rusqlite::Connection::open(&path).unwrap();
            let (id, answer) = if kind == "hermes" {
                conn.execute_batch(include_str!(
                    "../../repomon-core/src/usage_ledger/fixtures/hermes_conversation_v0.sql"
                ))
                .unwrap();
                (
                    "hermes-fixture",
                    "The Hermes fixture repository inspection is complete",
                )
            } else {
                conn.execute_batch(include_str!(
                    "../../repomon-core/src/usage_ledger/fixtures/opencode_conversation_v0.sql"
                ))
                .unwrap();
                conn.execute("UPDATE part SET data=?1 WHERE id='prose'", [r#"{"type":"text","text":"Code compiles, tests pass green,\nDeploy succeeds, the pipeline clean."}"#]).unwrap();
                (
                    "first",
                    "Code compiles, tests pass green,         Context 1%\nDeploy succeeds, the pipeline clean.    MCP connected",
                )
            };
            let backend = ScriptedBackend::default();
            let start = DateTime::from_timestamp(0, 0).unwrap();
            *backend.started.lock().unwrap() = Some(Some(start));
            *backend.pane.lock().unwrap() = answer.into();
            backend.metas.lock().unwrap().push(WindowMeta {
                name: "lane-1".into(),
                wid: 1,
                session: None,
                agent_kind: Some(kind.into()),
            });
            let src = Source {
                window: String::new(),
                kind: kind.into(),
                path: Some(path),
                session: Some(id.into()),
            };
            let request = || Request {
                window: "lane-1",
                kind,
                cwd: dir.path(),
                bound: None,
                started: start,
            };
            assert_eq!(
                select(&Cache::default(), &backend, request(), vec![src.clone()])
                    .unwrap()
                    .session
                    .as_deref(),
                Some(id)
            );
            backend.metas.lock().unwrap()[0].session = None;
            *backend.pane.lock().unwrap() = "hi".into();
            assert!(select(&Cache::default(), &backend, request(), vec![src]).is_none());
        }
    }
    #[test]
    fn per_kind_identity_uses_unique_pane_evidence_and_never_latest_cwd() {
        for kind in ["claude-code", "codex", "antigravity"] {
            let dir = tempfile::tempdir().unwrap();
            let cache = Cache::default();
            let backend = ScriptedBackend::default();
            let start = DateTime::parse_from_rfc3339("2026-09-11T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            *backend.started.lock().unwrap() = Some(Some(start));
            backend.metas.lock().unwrap().push(WindowMeta {
                name: "lane-1".into(),
                wid: 1,
                session: None,
                agent_kind: Some(kind.into()),
            });
            let wanted = fixture(
                dir.path(),
                kind,
                "wanted",
                "A distinctive fixture answer belonging to the selected window",
            );
            let other = fixture(
                dir.path(),
                kind,
                "other",
                "A newer unrelated answer from another concurrent session",
            );
            *backend.pane.lock().unwrap() =
                "A distinctive fixture answer belonging to the selected window".into();
            let request = || Request {
                window: "lane-1",
                kind,
                cwd: dir.path(),
                bound: None,
                started: start,
            };
            let found = select(
                &cache,
                &backend,
                request(),
                vec![other.clone(), wanted.clone()],
            )
            .unwrap();
            assert_eq!(found.session.as_deref(), Some("wanted"), "{kind}");
            assert_eq!(
                backend.metas.lock().unwrap()[0].session.as_deref(),
                Some("wanted")
            );
            backend.metas.lock().unwrap()[0].session = None;
            assert!(
                select(&cache, &backend, request(), vec![other]).is_none(),
                "no cwd fallback for {kind}"
            );
            let duplicate = fixture(
                dir.path(),
                kind,
                "duplicate",
                "A distinctive fixture answer belonging to the selected window",
            );
            assert!(
                select(&cache, &backend, request(), vec![wanted.clone(), duplicate]).is_none(),
                "ambiguous sessions for {kind}"
            );
            backend.metas.lock().unwrap().push(WindowMeta {
                name: "lane-2".into(),
                wid: 2,
                session: None,
                agent_kind: Some(kind.into()),
            });
            assert!(
                select(&cache, &backend, request(), vec![wanted.clone()]).is_none(),
                "same evidence in two windows for {kind}"
            );
            backend.metas.lock().unwrap().pop();
            let late = DateTime::parse_from_rfc3339("2026-09-11T13:00:00Z")
                .unwrap()
                .with_timezone(&Utc);
            *backend.started.lock().unwrap() = Some(Some(late));
            assert!(
                select(
                    &cache,
                    &backend,
                    Request {
                        started: late,
                        ..request()
                    },
                    vec![wanted]
                )
                .is_none(),
                "pre-window history for {kind}"
            );
        }
    }
    #[test]
    fn codex_meta_records_its_lane_instead_of_using_the_filename_as_identity() {
        let dir = tempfile::tempdir().unwrap();
        let src = fixture(
            dir.path(),
            "codex",
            "session-id",
            "A fixture answer from the right rollout and working directory",
        );
        let (id, cwd) = codex_meta(src.path.as_deref().unwrap()).unwrap();
        assert_eq!(id, "session-id");
        assert_eq!(cwd.as_deref(), Some(dir.path()));
    }
    #[test]
    fn antigravity_worktree_expands_to_main_checkout_but_still_requires_window_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("main");
        let wt = dir.path().join("worktree");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(main.join(".git/worktrees/lane")).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        // Use real git plumbing with an isolated repository to validate linked-worktree identity.
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&main)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "fixture"
                ])
                .status()
                .unwrap()
                .success()
        );
        std::fs::remove_dir(&wt).unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(["worktree", "add", "--detach", "--quiet"])
                .arg(&wt)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            main_checkout(&wt).unwrap().canonicalize().unwrap(),
            main.canonicalize().unwrap()
        );
    }
}
