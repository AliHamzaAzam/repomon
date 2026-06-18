//! Parsing Claude Code's interactive `/usage` screen into structured limit numbers.
//!
//! Claude exposes subscription usage *only* through the interactive `/usage` command — there's no
//! CLI flag, local file, or supported endpoint. So the daemon's usage probe runs `/usage` in a
//! throwaway session, captures the pane (`capture-pane -e`), and hands the text here. This module
//! is the pure, fixture-tested heart: it strips ANSI, anchors on the section labels Claude prints
//! ("Current session", "Current week (all models)"), and reads each section's percentage and
//! reset time. It is deliberately lenient about layout — the `/usage` screen is undocumented and
//! changes between versions, so it anchors on labels rather than line positions and returns
//! `None` (never fabricated zeros) when nothing recognizable is on screen.

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use super::text::{parse_pct, parse_reset_datetime, strip_ansi};

/// Claude account usage, parsed from `/usage`. Every field is optional so a partial screen still
/// yields what was readable. Percentages are "% used" of each window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    /// The 5-hour ("Current session") window, % used.
    pub session_pct: Option<u8>,
    /// When the 5-hour window resets.
    pub session_reset_at: Option<DateTime<Utc>>,
    /// The weekly ("Current week (all models)") window, % used.
    pub week_pct: Option<u8>,
    /// When the weekly window resets (a dated time, days out).
    pub week_reset_at: Option<DateTime<Utc>>,
    /// The model-specific weekly window, % used (e.g. "Current week (Opus)" / "(Sonnet only)").
    pub week_model_pct: Option<u8>,
}

impl UsageReport {
    /// Whether anything usable was parsed (at least one percentage).
    pub fn is_empty(&self) -> bool {
        self.session_pct.is_none() && self.week_pct.is_none() && self.week_model_pct.is_none()
    }
}

/// One Claude account's usage, as carried over RPC to clients. `key` matches
/// [`super::claude::account_key`] so a client can pick the account of the focused agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUsage {
    /// Stable account key (the canonical config dir, or "default").
    pub key: String,
    /// Short human label ("main", "work", …).
    pub label: String,
    pub report: UsageReport,
    /// How long ago the probe captured this, in seconds.
    pub age_secs: u64,
}

/// Parse the `/usage` screen. Returns `None` when no percentage is found anywhere — a blank,
/// loading, or trust/onboarding screen yields nothing rather than fake numbers.
pub fn parse_usage(pane: &str) -> Option<UsageReport> {
    let lines: Vec<String> = pane.lines().map(strip_ansi).collect();
    let now = Local::now();
    let mut r = UsageReport::default();

    for (i, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        if low.contains("current session") {
            let (pct, reset) = section_after(&lines, i, now);
            r.session_pct = pct;
            r.session_reset_at = reset;
        } else if low.contains("current week") {
            if low.contains("all models") {
                let (pct, reset) = section_after(&lines, i, now);
                r.week_pct = pct;
                r.week_reset_at = reset;
            } else {
                // A model-specific weekly window: "(Opus)", "(Sonnet only)", etc.
                let (pct, _) = section_after(&lines, i, now);
                r.week_model_pct = pct;
            }
        }
    }

    (!r.is_empty()).then_some(r)
}

/// Read a section's `NN% used` and `Resets …` from the few lines following its header. Stops at
/// the next section header so one section never borrows another's numbers.
fn section_after(
    lines: &[String],
    header: usize,
    now: DateTime<Local>,
) -> (Option<u8>, Option<DateTime<Utc>>) {
    let mut pct = None;
    let mut reset = None;
    for line in lines.iter().skip(header + 1).take(4) {
        let low = line.to_lowercase();
        if low.contains("current session") || low.contains("current week") {
            break;
        }
        if pct.is_none() {
            pct = parse_pct(line);
        }
        if reset.is_none() && low.contains("reset") {
            reset = parse_reset_datetime(&low, now);
        }
    }
    (pct, reset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn parses_real_usage_capture() {
        // The real `/usage` screen from Claude Code v2.1.181, captured via `capture-pane -e`
        // (so it carries ANSI color escapes — this also exercises strip_ansi).
        let pane = include_str!("fixtures/usage_v2.ansi");
        let r = parse_usage(pane).expect("should parse the /usage screen");
        assert_eq!(r.session_pct, Some(15));
        assert_eq!(r.week_pct, Some(41));
        assert_eq!(r.week_model_pct, Some(0));

        let session = r
            .session_reset_at
            .expect("session reset")
            .with_timezone(&Local);
        assert_eq!((session.hour(), session.minute()), (23, 59)); // "Resets 11:59pm"

        let week = r.week_reset_at.expect("week reset").with_timezone(&Local);
        assert_eq!((week.month(), week.day()), (6, 21)); // "Resets Jun 21 at 7:59pm"
        assert_eq!((week.hour(), week.minute()), (19, 59));
    }

    #[test]
    fn trust_prompt_yields_none() {
        // The folder-trust prompt has no usage numbers — must not parse as zeros.
        let pane = include_str!("fixtures/trust_prompt.txt");
        assert!(parse_usage(pane).is_none());
    }

    #[test]
    fn blank_and_ordinary_output_yield_none() {
        assert!(parse_usage("").is_none());
        assert!(parse_usage("running 24 tests\ntest result: ok. 24 passed").is_none());
    }

    #[test]
    fn partial_screen_session_only() {
        let pane = "Current session\n  ████ 22% used\n  Resets 3:00pm\n";
        let r = parse_usage(pane).expect("partial parse");
        assert_eq!(r.session_pct, Some(22));
        assert_eq!(r.week_pct, None);
        assert_eq!(r.week_model_pct, None);
        assert!(r.session_reset_at.is_some());
    }

    #[test]
    fn sections_do_not_borrow_each_others_numbers() {
        // No percentage under "Current session" → it must stay None, not grab the week's 88%.
        let pane = "Current session\n  Resets 3:00pm\n\nCurrent week (all models)\n  88% used\n";
        let r = parse_usage(pane).expect("week parsed");
        assert_eq!(r.session_pct, None);
        assert_eq!(r.week_pct, Some(88));
    }
}
