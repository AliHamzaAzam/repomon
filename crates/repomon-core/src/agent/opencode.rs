//! Reads OpenCode sessions from SQLite after validating required tables and columns, returning no
//! summary for incompatible schemas.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use super::TranscriptSummary;
use crate::model::{AgentKind, AgentStatus};

const IDLE_AFTER: Duration = Duration::minutes(2);

pub fn database_path() -> PathBuf {
    if let Ok(path) = std::env::var("REPOMON_OPENCODE_DB") {
        return PathBuf::from(path);
    }
    if let Ok(root) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(root).join("opencode/opencode.db");
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".local/share/opencode/opencode.db"))
        .unwrap_or_else(|| PathBuf::from(".local/share/opencode/opencode.db"))
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect()
}

fn compatible(conn: &Connection) -> bool {
    let Ok(session) = columns(conn, "session") else {
        return false;
    };
    let Ok(message) = columns(conn, "message") else {
        return false;
    };
    let Ok(part) = columns(conn, "part") else {
        return false;
    };
    ["id", "directory", "title", "time_updated", "time_archived"]
        .iter()
        .all(|name| session.contains(*name))
        && ["id", "session_id", "time_created", "data"]
            .iter()
            .all(|name| message.contains(*name))
        && ["session_id", "data"]
            .iter()
            .all(|name| part.contains(*name))
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}

type FileStamp = Option<(SystemTime, u64)>;

fn file_stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn database_stamp(path: &Path) -> (FileStamp, FileStamp) {
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    (file_stamp(path), file_stamp(Path::new(&wal)))
}

/// Whether a database stamp may be memoised. SQLite commits into recycled WAL frames, so a commit
/// can leave the WAL's length identical; if its timestamp has not settled either, the pair is not
/// evidence of anything. An absent file is stable: it can only start existing, which shows up as a
/// stamp changing from `None`.
fn stamp_is_settled(stamp: &(FileStamp, FileStamp), observed_at: SystemTime) -> bool {
    [&stamp.0, &stamp.1].into_iter().all(|file| match file {
        Some((modified, _)) => crate::fs_stamp::is_settled(*modified, observed_at),
        None => true,
    })
}

#[derive(Hash, PartialEq, Eq)]
struct SummaryKey {
    path: PathBuf,
    cwd: PathBuf,
    within: Duration,
    max: usize,
}

struct SummaryEntry {
    stamp: (FileStamp, FileStamp),
    summaries: Vec<TranscriptSummary>,
}

/// Reuse unchanged database reads. Opening even a read-only SQLite connection can rebuild the
/// WAL shared-memory index, so avoid opening it once per lane on every idle overlay tick.
pub fn summaries_for(cwd: &Path, within: Duration, max: usize) -> Vec<TranscriptSummary> {
    summaries_at(&database_path(), cwd, within, max)
}

fn summaries_at(path: &Path, cwd: &Path, within: Duration, max: usize) -> Vec<TranscriptSummary> {
    static CACHE: OnceLock<Mutex<HashMap<SummaryKey, SummaryEntry>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    let key = SummaryKey {
        path: path.into(),
        cwd: cwd.into(),
        within,
        max,
    };
    let stamp = database_stamp(path);
    // Read before the database read below, never after.
    let observed_at = SystemTime::now();
    // A missing database has no sessions and must not be created by observation.
    if stamp.0.is_none() {
        return Vec::new();
    }
    let mut entries = cache.lock().unwrap_or_else(|error| error.into_inner());
    let summaries = if let Some(entry) = entries.get(&key).filter(|entry| entry.stamp == stamp) {
        entry.summaries.clone()
    } else {
        let Some(summaries) = read_summaries(path, cwd, within, max) else {
            return Vec::new();
        };
        // A concurrent commit must cause another read on the next overlay.
        if database_stamp(path) == stamp && stamp_is_settled(&stamp, observed_at) {
            if entries.len() >= 256 {
                entries.clear();
            }
            entries.insert(
                key,
                SummaryEntry {
                    stamp,
                    summaries: summaries.clone(),
                },
            );
        }
        summaries
    };
    drop(entries);
    let now = Utc::now();
    summaries
        .into_iter()
        .filter(|summary| now - summary.last_activity <= within)
        .map(|mut summary| {
            if now - summary.last_activity > IDLE_AFTER {
                summary.status = AgentStatus::Idle;
            }
            summary
        })
        .collect()
}

fn read_summaries(
    path: &Path,
    cwd: &Path,
    within: Duration,
    max: usize,
) -> Option<Vec<TranscriptSummary>> {
    let Ok(conn) = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return None;
    };
    if !compatible(&conn) {
        return None;
    }
    let cutoff = (Utc::now() - within).timestamp_millis();
    let Ok(mut statement) = conn.prepare(
        "SELECT id, directory, title, time_updated FROM session \
         WHERE time_archived IS NULL AND time_updated >= ?1 ORDER BY time_updated DESC",
    ) else {
        return None;
    };
    let Ok(rows) = statement.query_map(params![cutoff], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    }) else {
        return None;
    };
    Some(
        rows.flatten()
            .filter(|(_, directory, _, _)| same_path(Path::new(directory), cwd))
            .take(max)
            .filter_map(|(id, _, title, updated)| summarize(&conn, path, id, title, updated))
            .collect(),
    )
}

/// Enumerate identities for transcript discovery without computing usage or choosing a winner.
pub fn session_ids_since(cwd: &Path, since: DateTime<Utc>) -> Vec<String> {
    session_ids_at(&database_path(), cwd, since).unwrap_or_default()
}
fn session_ids_at(path: &Path, cwd: &Path, since: DateTime<Utc>) -> Option<Vec<String>> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    if !compatible(&conn) {
        return None;
    }
    let mut stmt = conn
        .prepare(
            "SELECT id,directory FROM session WHERE time_archived IS NULL AND time_updated >= ?1",
        )
        .ok()?;
    let rows = stmt
        .query_map([since.timestamp_millis()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .ok()?;
    Some(
        rows.flatten()
            .filter(|(_, path)| same_path(Path::new(path), cwd))
            .map(|(id, _)| id)
            .collect(),
    )
}

pub fn summary_for(cwd: &Path) -> Option<TranscriptSummary> {
    summaries_for(cwd, Duration::hours(6), 1).into_iter().next()
}

fn summarize(
    conn: &Connection,
    path: &Path,
    session_id: String,
    title: Option<String>,
    updated: i64,
) -> Option<TranscriptSummary> {
    let last_activity = DateTime::<Utc>::from_timestamp_millis(updated)?;
    let latest: Option<String> = conn
        .query_row(
            "SELECT data FROM message WHERE session_id = ?1 ORDER BY time_created DESC LIMIT 1",
            params![session_id],
            |row| row.get(0),
        )
        .ok();
    let latest = latest.and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let running_tool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM part WHERE session_id = ?1 \
             AND json_extract(data, '$.type') = 'tool' \
             AND json_extract(data, '$.state.status') IN ('pending','running'))",
            params![session_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);
    let tool_call_count = conn
        .query_row(
            "SELECT COUNT(*) FROM part WHERE session_id = ?1 \
             AND json_extract(data, '$.type') = 'tool'",
            params![session_id],
            |row| row.get::<_, u32>(0),
        )
        .unwrap_or(0);
    let assistant = latest
        .as_ref()
        .and_then(|value| value.get("role"))
        .and_then(Value::as_str)
        == Some("assistant");
    let completed = latest
        .as_ref()
        .and_then(|value| value.pointer("/time/completed"))
        .is_some_and(|value| !value.is_null());
    let error = latest
        .as_ref()
        .and_then(|value| value.get("error"))
        .is_some_and(|value| !value.is_null());
    let ended_turn = assistant && completed && !running_tool;
    let status = if Utc::now() - last_activity > IDLE_AFTER {
        AgentStatus::Idle
    } else if ended_turn || error {
        AgentStatus::Waiting
    } else {
        AgentStatus::Running
    };
    // Retain assistant text so pane fingerprints can prove the session’s managed-window identity.
    let last_message = latest_text(conn, &session_id).map(|t| truncate(&t, 200));
    Some(TranscriptSummary {
        kind: AgentKind::OpenCode,
        manifest_path: path.to_path_buf(),
        cwd: None,
        last_activity,
        tool_call_count,
        status,
        title,
        last_message: error
            .then(|| "OpenCode session ended with an error".to_string())
            .or(last_message),
        config_dir: None,
        session_id: Some(session_id),
        ended_turn,
    })
}

/// The newest assistant message text for a session (its parts carry the actual prose), or None.
fn latest_text(conn: &Connection, session_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT p.data FROM part p JOIN message m ON p.message_id = m.id \
         WHERE p.session_id = ?1 AND json_extract(m.data, '$.role') = 'assistant' \
         AND json_extract(p.data, '$.type') = 'text' \
         ORDER BY p.time_created DESC LIMIT 1",
        params![session_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    .and_then(|value| {
        value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_candidates_include_all_recent_cwd_sessions_and_exclude_foreign_or_old() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.db");
        fixture(&path, dir.path());
        let conn = Connection::open(&path).unwrap();
        let since = Utc::now() - Duration::minutes(1);
        conn.execute("INSERT INTO session SELECT 'second',directory,title,time_updated,NULL FROM session WHERE id='ses-1'",[]).unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('foreign','/elsewhere','title',?1,NULL)",
            [Utc::now().timestamp_millis()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session VALUES ('old',?1,'title',1,NULL)",
            [dir.path().to_string_lossy().as_ref()],
        )
        .unwrap();
        let mut ids = session_ids_at(&path, dir.path(), since).unwrap();
        ids.sort();
        assert_eq!(ids, vec!["second", "ses-1"]);
    }
    #[test]
    fn cached_summaries_follow_wal_commits_and_database_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.db");
        fixture(&db, dir.path());
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let read = || summaries_at(&db, dir.path(), Duration::hours(6), 10);
        assert_eq!(read()[0].title.as_deref(), Some("Fix tests"));
        conn.execute("UPDATE session SET title = 'changed in WAL'", [])
            .unwrap();
        assert_eq!(read()[0].title.as_deref(), Some("changed in WAL"));
        assert_eq!(read()[0].title.as_deref(), Some("changed in WAL"));
        conn.execute(
            "UPDATE session SET time_updated = ?1",
            params![(Utc::now() - Duration::minutes(3)).timestamp_millis()],
        )
        .unwrap();
        assert_eq!(read()[0].status, AgentStatus::Idle);
        drop(conn);
        std::fs::remove_file(&db).unwrap();
        assert!(read().is_empty());
        fixture(&db, dir.path());
        assert_eq!(read()[0].title.as_deref(), Some("Fix tests"));
    }

    /// Serialize all environment access in these tests so path overrides cannot race parallel
    /// readers.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_db<T>(db: &Path, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: single-threaded within the lock; nothing else reads the variable here.
        unsafe { std::env::set_var("REPOMON_OPENCODE_DB", db) };
        let out = f();
        // SAFETY: as above.
        unsafe { std::env::remove_var("REPOMON_OPENCODE_DB") };
        out
    }

    fn fixture(path: &Path, cwd: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session(id TEXT, directory TEXT, title TEXT, time_updated INTEGER, time_archived INTEGER);\n\
             CREATE TABLE message(id TEXT, session_id TEXT, time_created INTEGER, data TEXT);\n\
             CREATE TABLE part(id TEXT, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO session VALUES('ses-1', ?1, 'Fix tests', ?2, NULL)",
            params![cwd.to_string_lossy(), now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO message VALUES('msg-1', 'ses-1', ?1, ?2)",
            params![now, r#"{"role":"assistant","time":{"completed":1}}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES('part-0', 'msg-1', 'ses-1', ?1, ?2)",
            params![now - 10, r#"{"type":"text","text":"Working on it now."}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES('part-1', 'msg-1', 'ses-1', ?1, ?2)",
            params![now, r#"{"type":"tool","state":{"status":"completed"}}"#],
        )
        .unwrap();
    }

    #[test]
    fn reads_compatible_store_and_detects_waiting() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("opencode.db");
        fixture(&db, temp.path());
        let summary = with_db(&db, || summary_for(temp.path()).unwrap());
        assert_eq!(summary.kind, AgentKind::OpenCode);
        assert_eq!(summary.status, AgentStatus::Waiting);
        assert!(summary.ended_turn);
        // The latest assistant text is surfaced so the daemon can fingerprint the transcript
        // against its pane capture (and notifications can show why).
        assert_eq!(summary.last_message.as_deref(), Some("Working on it now."));
        assert_eq!(summary.tool_call_count, 1);
    }

    #[test]
    fn last_message_is_truncated() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("opencode.db");
        fixture(&db, temp.path());
        let long = "x".repeat(400);
        let conn = Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO part VALUES('part-long', 'msg-1', 'ses-1', ?1, ?2)",
            params![
                Utc::now().timestamp_millis() - 5,
                format!(r#"{{"type":"text","text":"{long}"}}"#)
            ],
        )
        .unwrap();
        drop(conn);
        let summary = with_db(&db, || summary_for(temp.path()).unwrap());
        let msg = summary.last_message.unwrap();
        assert_eq!(msg.chars().count(), 200);
        assert!(msg.ends_with("..."));
    }

    #[test]
    fn user_messages_never_become_the_last_assistant_word() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("opencode.db");
        fixture(&db, temp.path());

        let conn = Connection::open(&db).unwrap();
        let now = Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO message VALUES('msg-2', 'ses-1', ?1, ?2)",
            params![now + 5, r#"{"role":"user"}"#],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES('part-user', 'msg-2', 'ses-1', ?1, ?2)",
            params![now + 5, r#"{"type":"text","text":"please ignore this"}"#],
        )
        .unwrap();
        drop(conn);
        let summary = with_db(&db, || summary_for(temp.path()).unwrap());
        assert_eq!(summary.last_message.as_deref(), Some("Working on it now."));
    }

    #[test]
    fn incompatible_store_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("opencode.db");
        Connection::open(&db)
            .unwrap()
            .execute("CREATE TABLE session(id TEXT)", [])
            .unwrap();
        with_db(&db, || assert!(summary_for(temp.path()).is_none()));
    }
}
