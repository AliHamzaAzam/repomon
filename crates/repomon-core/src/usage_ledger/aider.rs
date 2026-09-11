//! Aider's dated chat-history sections (aider/io.py user_input and ai_output). Undated files have no usable session boundary.
use super::scan::{ScanOptions, ScannedSession, SourceScan, TranscriptEntry};
use crate::{Result, model::TranscriptItem};
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use std::{
    io::{BufRead, BufReader},
    path::Path,
};

pub fn scan(
    path: &Path,
    session: Option<&str>,
    from: u64,
    options: ScanOptions,
) -> Result<SourceScan> {
    let reader = BufReader::new(std::fs::File::open(path)?);
    let mut out = SourceScan::default();
    let mut offset = 0u64;
    let mut id = None;
    let mut at: Option<DateTime<Utc>> = None;
    let mut pending: Option<TranscriptEntry> = None;
    let flush = |pending: &mut Option<TranscriptEntry>,
                 out: &mut SourceScan,
                 id: &Option<String>| {
        if let Some(mut row) = pending.take() {
            row.item.text = row.item.text.trim().into();
            if options.collect_transcript
                && session.is_none_or(|s| id.as_deref() == Some(s))
                && row.offset >= from
                && options.before_offset.is_none_or(|end| row.offset < end)
                && !row.item.text.is_empty()
            {
                if row.item.role == "user" {
                    for (n, mut item) in crate::agent::repomail::split(&row.item.text, row.item.at)
                        .into_iter()
                        .enumerate()
                    {
                        item.id = Some(format!("{}:{n}", row.offset));
                        out.transcript.push(TranscriptEntry {
                            offset: row.offset,
                            item,
                        });
                    }
                } else {
                    out.transcript.push(row);
                }
            }
        }
    };
    for line in reader.split(b'\n') {
        let line = line?;
        let text = String::from_utf8_lossy(&line);
        if let Some(date) = text.strip_prefix("# aider chat started at ") {
            flush(&mut pending, &mut out, &id);
            at = NaiveDateTime::parse_from_str(date.trim(), "%Y-%m-%d %H:%M:%S")
                .ok()
                .and_then(|dt| Local.from_local_datetime(&dt).single())
                .map(|dt| dt.with_timezone(&Utc));
            id = at.map(|at| format!("aider:{}:{offset}", at.timestamp()));
            if let Some(id) = &id {
                out.sessions.push(ScannedSession {
                    session_id: id.clone(),
                    agent_kind: "aider".into(),
                    cwd: path.parent().map(|p| p.to_string_lossy().into()),
                    first_at: at,
                    last_at: at,
                    headline: None,
                    headline_raw: None,
                    turns: 0,
                    tool_calls: 0,
                    retries: 0,
                    subagent: false,
                });
            }
        } else if at.is_some() {
            if let Some(user) = text.strip_prefix("#### ") {
                if let Some(row) = pending.as_mut().filter(|r| r.item.role == "user") {
                    row.item.text.push('\n');
                    row.item.text.push_str(user.trim_end());
                    offset += line.len() as u64 + 1;
                    continue;
                }
                flush(&mut pending, &mut out, &id);
                let mut item = TranscriptItem::new("user", user.trim_end(), at);
                item.id = Some(format!("{offset}:0"));
                pending = Some(TranscriptEntry { offset, item });
            } else if !text.starts_with("> ") {
                // Aider writes each user prompt with #### prefixes and assistant prose after it.
                if pending.as_ref().is_some_and(|r| r.item.role == "user") && text.trim().is_empty()
                {
                    flush(&mut pending, &mut out, &id);
                } else {
                    let row = pending.get_or_insert_with(|| {
                        let mut item = TranscriptItem::new("assistant", "", at);
                        item.id = Some(format!("{offset}:0"));
                        TranscriptEntry { offset, item }
                    });
                    row.item.text.push_str(&text);
                    row.item.text.push('\n');
                }
            }
        }
        offset += line.len() as u64 + 1;
    }
    flush(&mut pending, &mut out, &id);
    out.next_offset = offset;
    out.sessions
        .retain(|s| session.is_none_or(|id| id == s.session_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dated_sections_are_separate_and_undated_history_is_not_claimed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".aider.chat.history.md");
        std::fs::write(&path, include_str!("fixtures/aider_conversation_v0.md")).unwrap();
        let all = scan(&path, None, 0, ScanOptions::default()).unwrap();
        assert_eq!(all.sessions.len(), 2);
        let first = scan(
            &path,
            Some(&all.sessions[0].session_id),
            0,
            ScanOptions {
                collect_transcript: true,
                before_offset: None,
            },
        )
        .unwrap();
        assert_eq!(first.transcript.len(), 2);
        assert_eq!(first.transcript[0].item.role, "user");
        assert!(
            !first
                .transcript
                .iter()
                .any(|r| r.item.text.contains("unrelated"))
        );
        std::fs::write(&path, "#### Undated question\n\nAnswer\n").unwrap();
        assert!(
            scan(
                &path,
                None,
                0,
                ScanOptions {
                    collect_transcript: true,
                    before_offset: None
                }
            )
            .unwrap()
            .transcript
            .is_empty()
        );
    }
}
