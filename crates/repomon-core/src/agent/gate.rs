//! Reading a dxkit stop-gate verdict from a worktree's loop ledger.
//!
//! [dxkit](https://github.com/vyuh-labs/dxkit) is a deterministic Stop-hook for Claude Code:
//! when an agent tries to declare "done", the gate reruns scanners/tests on the changed files
//! and blocks the stop on net-new findings. Every gate run appends one JSON line to
//! `.dxkit/loop/ledger.jsonl` in the worktree (an append-only audit trail dxkit documents as
//! safe for external tools to read). repomon tails that file: a fresh `allowed` verdict is a
//! stronger done-signal than the git heuristic, and a fresh block *vetoes* it — the gate
//! explicitly said the work isn't done. Worktrees without dxkit simply have no ledger, and
//! everything falls back to the git heuristic.

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Relative location of dxkit's loop ledger inside a worktree.
pub const LEDGER_REL: &str = ".dxkit/loop/ledger.jsonl";

/// How many bytes of the ledger tail to read — events are single lines well under 1 KB, so
/// this always covers the last event without reading an unbounded audit trail.
const TAIL_BYTES: u64 = 8 * 1024;

/// The latest stop-gate verdict for a worktree, as read from the dxkit loop ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateVerdict {
    /// Whether the gate let the agent stop (`false` = bounced on net-new findings).
    pub allowed: bool,
    /// Net-new findings that blocked completion (0 when allowed).
    pub net_new_findings: u32,
    /// When the gate ran.
    pub at: DateTime<Utc>,
    /// The Claude session the verdict belongs to, when the hook payload carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// Parse the LAST well-formed stop-gate event out of ledger content (later lines win —
/// the ledger is append-only). Unknown fields, junk lines, and future schema versions are
/// skipped rather than errors: the ledger is another tool's file.
pub fn parse_ledger_tail(content: &str) -> Option<GateVerdict> {
    #[derive(Deserialize)]
    struct Line {
        event: String,
        timestamp: DateTime<Utc>,
        allowed: bool,
        #[serde(default)]
        net_new_findings: u32,
        #[serde(default)]
        session_id: Option<String>,
    }
    content.lines().rev().find_map(|l| {
        let line: Line = serde_json::from_str(l.trim()).ok()?;
        (line.event == "Stop").then_some(GateVerdict {
            allowed: line.allowed,
            net_new_findings: line.net_new_findings,
            at: line.timestamp,
            session_id: line.session_id,
        })
    })
}

/// Read the latest gate verdict from `worktree`'s dxkit ledger, if one exists. Reads only
/// the file's tail — the ledger is an unbounded audit trail.
pub fn read_gate_verdict(worktree: &Path) -> Option<GateVerdict> {
    use std::io::{Read, Seek, SeekFrom};
    let path = worktree.join(LEDGER_REL);
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = String::new();
    f.read_to_string(&mut tail).ok()?;
    // A mid-line seek start means the first line is a fragment; the parser skips junk lines.
    parse_ledger_tail(&tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A real-shaped dxkit ledger line (schema_version 1), trimmed to the fields that matter.
    const BLOCKED: &str = r#"{"schema_version":1,"timestamp":"2026-07-07T10:15:30Z","event":"Stop","session_id":"abc-123","cwd":"/w","branch":"feat/x","commit":"deadbeef","guardrail_status":"fail","net_new_findings":2,"baseline_findings":14,"files_changed":3,"allowed":false,"stop_hook_active":false,"tests_status":"pass","lint_status":"pass","typecheck_status":"pass","duration_ms":1200}"#;
    const ALLOWED: &str = r#"{"schema_version":1,"timestamp":"2026-07-07T10:22:05Z","event":"Stop","session_id":"abc-123","cwd":"/w","branch":"feat/x","commit":"deadbeef","guardrail_status":"pass","net_new_findings":0,"baseline_findings":14,"files_changed":3,"allowed":true,"stop_hook_active":true,"tests_status":"pass","lint_status":"pass","typecheck_status":"pass","duration_ms":900}"#;

    #[test]
    fn last_event_wins_and_junk_is_skipped() {
        // Blocked then allowed (the repair loop finishing): the tail verdict is the allowed one.
        let content = format!("{BLOCKED}\n{ALLOWED}\n");
        let v = parse_ledger_tail(&content).expect("verdict");
        assert!(v.allowed);
        assert_eq!(v.net_new_findings, 0);
        assert_eq!(v.session_id.as_deref(), Some("abc-123"));
        assert_eq!(
            v.at,
            "2026-07-07T10:22:05Z".parse::<DateTime<Utc>>().unwrap()
        );

        // Allowed then blocked (a later attempt regressed): the block wins.
        let content = format!("{ALLOWED}\n{BLOCKED}\n");
        let v = parse_ledger_tail(&content).expect("verdict");
        assert!(!v.allowed);
        assert_eq!(v.net_new_findings, 2);

        // A trailing fragment (mid-line tail seek), junk, and blank lines are skipped.
        let content = format!("gs\":14}}\n\nnot json\n{ALLOWED}\n{{\"half\":");
        let v = parse_ledger_tail(&content).expect("verdict");
        assert!(v.allowed);
    }

    #[test]
    fn non_stop_events_and_empty_content_yield_none() {
        assert_eq!(parse_ledger_tail(""), None);
        assert_eq!(parse_ledger_tail("not json at all\n"), None);
        // A future non-Stop event kind is not a verdict.
        let other = r#"{"schema_version":1,"timestamp":"2026-07-07T10:00:00Z","event":"Baseline","allowed":true,"net_new_findings":0}"#;
        assert_eq!(parse_ledger_tail(other), None);
    }

    #[test]
    fn reads_the_tail_of_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = dir.path().join(LEDGER_REL);
        std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        std::fs::write(&ledger, format!("{BLOCKED}\n{ALLOWED}\n")).unwrap();
        let v = read_gate_verdict(dir.path()).expect("verdict");
        assert!(v.allowed);
        // No ledger at all → None (the common, dxkit-less case).
        let bare = tempfile::tempdir().unwrap();
        assert_eq!(read_gate_verdict(bare.path()), None);
    }
}
