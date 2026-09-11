//! Hermes TUI state.db reader. Message IDs are durable pagination cursors, not byte offsets.
use super::scan::{ScanOptions, ScannedSession, SourceScan};
use crate::error::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use std::path::{Path, PathBuf};

pub fn database_path() -> PathBuf {
    std::env::var_os("REPOMON_HERMES_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HERMES_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    directories::BaseDirs::new()
                        .map(|d| d.home_dir().join(".hermes"))
                        .unwrap_or_else(|| PathBuf::from(".hermes"))
                });
            home.join("state.db")
        })
}
fn open(path: &Path) -> Result<Connection> {
    Ok(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?)
}
fn stamp(seconds: f64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
}
pub fn sessions(path: &Path) -> Result<Vec<ScannedSession>> {
    let conn = open(path)?;
    let mut stmt = conn.prepare("SELECT s.id, s.cwd, s.started_at, COALESCE(MAX(m.timestamp),s.started_at) FROM sessions s LEFT JOIN messages m ON m.session_id=s.id GROUP BY s.id")?;
    Ok(stmt
        .query_map([], |r| {
            Ok(ScannedSession {
                session_id: r.get(0)?,
                agent_kind: "hermes".into(),
                cwd: r.get(1)?,
                first_at: stamp(r.get(2)?),
                last_at: stamp(r.get(3)?),
                headline: None,
                headline_raw: None,
                turns: 0,
                tool_calls: 0,
                retries: 0,
                subagent: false,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?)
}
pub fn page_bounds(path: &Path, session: &str, before: u64) -> Result<(u64, u64)> {
    let conn = open(path)?;
    let start: i64 = conn.query_row("SELECT COALESCE(MIN(id),0) FROM (SELECT id FROM messages WHERE session_id=?1 AND id<?2 ORDER BY id DESC LIMIT 200)", params![session, before.min(i64::MAX as u64) as i64], |r| r.get(0))?;
    let older: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE session_id=?1 AND id<?2",
        params![session, start],
        |r| r.get(0),
    )?;
    Ok((start as u64, older as u64))
}
pub fn scan(path: &Path, after: u64, session: &str, options: ScanOptions) -> Result<SourceScan> {
    let conn = open(path)?;
    let mut stmt = conn.prepare("SELECT m.id,m.role,m.content,m.tool_call_id,m.tool_calls,m.tool_name,m.timestamp,s.model FROM messages m JOIN sessions s ON s.id=m.session_id WHERE m.session_id=?1 AND m.id>?2 AND m.id<?3 ORDER BY m.id")?;
    let mut rows = stmt.query(params![
        session,
        after.min(i64::MAX as u64) as i64,
        options
            .before_offset
            .unwrap_or(i64::MAX as u64)
            .min(i64::MAX as u64) as i64
    ])?;
    let mut mapper = super::transcript::Mapper::default();
    let mut next = after;
    while let Some(r) = rows.next()? {
        let id: i64 = r.get(0)?;
        next = id as u64;
        if !options.collect_transcript {
            continue;
        }
        let content: Option<String> = r.get(2)?;
        let calls: Option<String> = r.get(4)?;
        let v = serde_json::json!({"role":r.get::<_,String>(1)?, "content":content.map(|s| serde_json::from_str::<serde_json::Value>(&s).ok().filter(|v| v.is_array()).unwrap_or(serde_json::Value::String(s))).unwrap_or_default(), "tool_call_id":r.get::<_,Option<String>>(3)?, "tool_calls":calls.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()), "tool_name":r.get::<_,Option<String>>(5)?, "timestamp":stamp(r.get(6)?), "model":r.get::<_,Option<String>>(7)?});
        mapper.hermes(&v, id);
    }
    Ok(SourceScan {
        transcript: mapper.rows,
        sessions: sessions(path)?
            .into_iter()
            .filter(|s| s.session_id == session)
            .collect(),
        next_offset: next,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_schema_sessions_tools_pagination_and_wal_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        writer
            .execute_batch(include_str!("fixtures/hermes_conversation_v0.sql"))
            .unwrap();
        let options = ScanOptions {
            collect_transcript: true,
            before_offset: None,
        };
        let first = scan(&path, 0, "hermes-fixture", options).unwrap();
        assert_eq!(
            first
                .transcript
                .iter()
                .filter(|r| r.item.kind.as_deref() == Some("user"))
                .count(),
            1
        );
        assert!(
            !first
                .transcript
                .iter()
                .any(|r| r.item.text.contains("Unrelated"))
        );
        assert!(
            first
                .transcript
                .iter()
                .any(|r| r.item.status == Some(crate::model::ToolCallStatus::Ok))
        );
        for i in 5..=209 {
            writer.execute("INSERT INTO messages (id,session_id,role,content,timestamp) VALUES (?1,'hermes-fixture','assistant','new reply',1789117210)",[i]).unwrap();
        }
        let (start, older) = page_bounds(&path, "hermes-fixture", u64::MAX).unwrap();
        assert_eq!(older, 8);
        let last = scan(&path, start - 1, "hermes-fixture", options).unwrap();
        assert_eq!(last.transcript.len(), 200);
        let previous = scan(
            &path,
            0,
            "hermes-fixture",
            ScanOptions {
                before_offset: Some(start),
                ..options
            },
        )
        .unwrap();
        assert!(previous.transcript.iter().all(|r| r.offset < start));
        assert_eq!(last.next_offset, 209);
    }
}
