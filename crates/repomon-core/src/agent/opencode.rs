//! Read-only OpenCode session monitor.
//!
//! OpenCode 1.15.5 stores sessions, messages, and parts in SQLite. The monitor validates the
//! required tables and columns before querying so a future incompatible schema degrades to no
//! summary instead of breaking the fleet overlay.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

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

pub fn summaries_for(cwd: &Path, within: Duration, max: usize) -> Vec<TranscriptSummary> {
    let path = database_path();
    let Ok(conn) = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    if !compatible(&conn) {
        return Vec::new();
    }
    let cutoff = (Utc::now() - within).timestamp_millis();
    let Ok(mut statement) = conn.prepare(
        "SELECT id, directory, title, time_updated FROM session \
         WHERE time_archived IS NULL AND time_updated >= ?1 ORDER BY time_updated DESC",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map(params![cutoff], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, i64>(3)?,
        ))
    }) else {
        return Vec::new();
    };
    rows.flatten()
        .filter(|(_, directory, _, _)| same_path(Path::new(directory), cwd))
        .take(max)
        .filter_map(|(id, _, title, updated)| summarize(&conn, &path, id, title, updated))
        .collect()
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
    Some(TranscriptSummary {
        kind: AgentKind::OpenCode,
        manifest_path: path.to_path_buf(),
        cwd: None,
        last_activity,
        tool_call_count,
        status,
        title,
        last_message: error.then(|| "OpenCode session ended with an error".to_string()),
        config_dir: None,
        session_id: Some(session_id),
        ended_turn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(path: &Path, cwd: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session(id TEXT, directory TEXT, title TEXT, time_updated INTEGER, time_archived INTEGER);\n\
             CREATE TABLE message(id TEXT, session_id TEXT, time_created INTEGER, data TEXT);\n\
             CREATE TABLE part(id TEXT, session_id TEXT, data TEXT);",
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
            "INSERT INTO part VALUES('part-1', 'ses-1', ?1)",
            params![r#"{"type":"tool","state":{"status":"completed"}}"#],
        )
        .unwrap();
    }

    #[test]
    fn reads_compatible_store_and_detects_waiting() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("opencode.db");
        fixture(&db, temp.path());
        unsafe { std::env::set_var("REPOMON_OPENCODE_DB", &db) };
        let summary = summary_for(temp.path()).unwrap();
        unsafe { std::env::remove_var("REPOMON_OPENCODE_DB") };
        assert_eq!(summary.kind, AgentKind::OpenCode);
        assert_eq!(summary.status, AgentStatus::Waiting);
        assert!(summary.ended_turn);
        assert_eq!(summary.tool_call_count, 1);
    }

    #[test]
    fn incompatible_store_returns_none() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("opencode.db");
        Connection::open(&db)
            .unwrap()
            .execute("CREATE TABLE session(id TEXT)", [])
            .unwrap();
        unsafe { std::env::set_var("REPOMON_OPENCODE_DB", &db) };
        assert!(summary_for(temp.path()).is_none());
        unsafe { std::env::remove_var("REPOMON_OPENCODE_DB") };
    }
}
