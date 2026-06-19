//! Parsing agent usage screens into structured limit "windows".
//!
//! Subscription usage has no CLI flag, file, or supported endpoint for either agent — the only
//! source is an interactive command: Claude's `/usage` and Codex's `/status`. The daemon's usage
//! probe runs that command in a throwaway session, captures the pane (`capture-pane -e`), and
//! hands the text here. This module is the pure, fixture-tested heart: it strips ANSI, anchors on
//! the labels each tool prints, and reads each limit window's percentage and reset time. It is
//! deliberately lenient about layout — these screens are undocumented and change between versions,
//! so it anchors on labels rather than positions and returns `None` (never fabricated numbers)
//! when nothing recognizable is on screen. Both tools are normalized to **% used** so the UI shows
//! one consistent metric (Codex reports "% left", which is converted).

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use super::text::{parse_pct, parse_relative_reset, parse_reset_datetime, strip_ansi};

/// One usage limit window, normalized across agents. `pct_used` is how much of the window is
/// consumed (0–100); `label` is a short tag for display (`5h`, `wk`, `mo`, or a model name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub label: String,
    pub pct_used: u8,
    pub reset_at: Option<DateTime<Utc>>,
}

/// An account's usage: an ordered list of limit windows (shortest first). Empty/absent windows
/// mean nothing was readable — clients show nothing rather than zeros.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReport {
    pub windows: Vec<UsageWindow>,
}

impl UsageReport {
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }
}

/// One account's usage, as carried over RPC to clients. `key` matches
/// [`super::claude::account_key`] (or `"codex"`) so a client can pick the focused agent's account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountUsage {
    pub key: String,
    pub label: String,
    pub report: UsageReport,
    /// How long ago the probe captured this, in seconds.
    pub age_secs: u64,
}

/// Parse Claude's `/usage` screen. Sections: "Current session" (the 5-hour window), "Current week
/// (all models)", and a model-specific weekly ("(Opus)"/"(Sonnet only)"). Returns `None` when no
/// percentage is found anywhere (a blank/loading/trust screen yields nothing, never fake zeros).
pub fn parse_usage(pane: &str) -> Option<UsageReport> {
    let lines: Vec<String> = pane.lines().map(strip_ansi).collect();
    let now = Local::now();
    let mut windows = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        if low.contains("current session") {
            if let Some((pct, reset)) = section_after(&lines, i, now) {
                windows.push(window("5h", pct, reset));
            }
        } else if low.contains("current week") {
            let label = if low.contains("all models") {
                "wk".to_string()
            } else {
                week_model_label(&low)
            };
            if let Some((pct, reset)) = section_after(&lines, i, now) {
                windows.push(window(&label, pct, reset));
            }
        }
    }

    (!windows.is_empty()).then_some(UsageReport { windows })
}

/// Parse Codex's `/status` screen. The lines look like
/// `│  Monthly limit:  [bars] 95% left (resets 04:00 on 19 Jul) │` — note Codex reports **% left**
/// (converted to % used here) and may show 5-hour, weekly, or (Free) monthly windows.
pub fn parse_codex_status(pane: &str) -> Option<UsageReport> {
    let now = Local::now();
    let mut windows = Vec::new();

    for line in pane.lines() {
        let clean = strip_ansi(line);
        let low = clean.to_lowercase();
        // Anchor on "<name> limit:"; the "rate limits and credits" hint has no colon and is skipped.
        let Some(lpos) = low.find("limit:") else {
            continue;
        };
        let Some(left) = parse_pct(&clean) else {
            continue;
        };
        let pct_used = 100u8.saturating_sub(left);
        let reset = if low.contains("reset") {
            parse_reset_datetime(&low, now)
        } else {
            None
        };
        windows.push(window(&codex_label(&low[..lpos]), pct_used, reset));
    }

    (!windows.is_empty()).then_some(UsageReport { windows })
}

/// Parse Gemini CLI's `/stats` (alias `/usage`) quota line. For OAuth Code-Assist accounts it
/// renders `"{N}% used (Limit resets in {dur})"`, or `"Limit reached, resets in {dur}"` when
/// exhausted, with `"Usage limit: {limit}"` / `"… reset daily"` detail rows. The window is the
/// daily request quota; the reset is a relative duration. (API-key/Vertex accounts show no quota,
/// so this yields `None` — the corner then falls back.) Normalized to % used (Gemini already is).
pub fn parse_gemini_status(pane: &str) -> Option<UsageReport> {
    let now = Local::now();
    let mut windows = Vec::new();
    for line in pane.lines() {
        let clean = strip_ansi(line);
        let low = clean.to_lowercase();
        if low.contains("limit reached") && low.contains("resets in") {
            windows.push(window("day", 100, parse_relative_reset(&low, now)));
        } else if low.contains("% used") && (low.contains("resets in") || low.contains("limit")) {
            if let Some(pct) = parse_pct(&clean) {
                let reset = if low.contains("resets in") {
                    parse_relative_reset(&low, now)
                } else {
                    None
                };
                windows.push(window("day", pct, reset));
            }
        }
    }
    (!windows.is_empty()).then_some(UsageReport { windows })
}

fn window(label: &str, pct_used: u8, reset_at: Option<DateTime<Utc>>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        pct_used,
        reset_at,
    }
}

/// Read a Claude section's `NN% used` and `Resets …` from the few lines after its header. Stops at
/// the next section header so one section never borrows another's numbers. `None` if no percentage.
fn section_after(
    lines: &[String],
    header: usize,
    now: DateTime<Local>,
) -> Option<(u8, Option<DateTime<Utc>>)> {
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
    pct.map(|p| (p, reset))
}

/// A short label for Claude's model-specific weekly window from its parenthetical, e.g.
/// `"current week (sonnet only)"` → `"sonnet"`, `"(opus)"` → `"opus"`.
fn week_model_label(low: &str) -> String {
    if let (Some(a), Some(b)) = (low.find('('), low.find(')')) {
        if b > a + 1 {
            if let Some(w) = low[a + 1..b].split_whitespace().next() {
                return w.to_string();
            }
        }
    }
    "wk2".to_string()
}

/// A short label for a Codex limit, from the text before "limit:" (e.g. `"│  monthly "` → `"mo"`).
fn codex_label(before_limit: &str) -> String {
    let n = before_limit.to_lowercase();
    if n.contains("5h") || n.contains("hour") {
        "5h".to_string()
    } else if n.contains("week") {
        "wk".to_string()
    } else if n.contains("month") {
        "mo".to_string()
    } else if n.contains("day") || n.contains("daily") {
        "day".to_string()
    } else {
        // Fall back to the last alphanumeric word before "limit:".
        n.split(|c: char| !c.is_alphanumeric())
            .rfind(|w| !w.is_empty())
            .unwrap_or("lim")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    fn win<'a>(r: &'a UsageReport, label: &str) -> &'a UsageWindow {
        r.windows
            .iter()
            .find(|w| w.label == label)
            .unwrap_or_else(|| panic!("expected a {label:?} window in {:?}", r.windows))
    }

    #[test]
    fn parses_real_claude_usage_capture() {
        // The real `/usage` screen from Claude Code v2.1.181, captured via `capture-pane -e`.
        let pane = include_str!("fixtures/usage_v2.ansi");
        let r = parse_usage(pane).expect("should parse the /usage screen");
        assert_eq!(win(&r, "5h").pct_used, 15);
        assert_eq!(win(&r, "wk").pct_used, 41);
        assert_eq!(win(&r, "sonnet").pct_used, 0); // "Current week (Sonnet only)"

        let s = win(&r, "5h").reset_at.unwrap().with_timezone(&Local);
        assert_eq!((s.hour(), s.minute()), (23, 59)); // "Resets 11:59pm"
        let w = win(&r, "wk").reset_at.unwrap().with_timezone(&Local);
        assert_eq!((w.month(), w.day()), (6, 21)); // "Resets Jun 21 at 7:59pm"
        assert_eq!((w.hour(), w.minute()), (19, 59));
    }

    #[test]
    fn parses_real_codex_status_capture() {
        // The real `/status` screen from Codex CLI v0.141.0 (Free plan: a monthly window).
        let pane = include_str!("fixtures/codex_status_v0.ansi");
        let r = parse_codex_status(pane).expect("should parse the /status screen");
        let mo = win(&r, "mo");
        assert_eq!(mo.pct_used, 5); // "95% left" → 5% used
        let reset = mo.reset_at.expect("monthly reset").with_timezone(&Local);
        assert_eq!((reset.month(), reset.day()), (7, 19)); // "resets 04:00 on 19 Jul"
        assert_eq!((reset.hour(), reset.minute()), (4, 0));
    }

    #[test]
    fn parses_gemini_quota_line() {
        // Gemini /stats renders this for OAuth Code-Assist accounts. The exact strings are from
        // gemini-cli's QuotaStatsInfo source; a live capture isn't possible here (a probe-spawned
        // gemini can't authenticate unattended), so this asserts the documented format.
        let pane = "Auth Method: oauth-personal\n  37% used (Limit resets in 3h 24m)\n  \
                    Usage limit: 1000\n  Usage limits span all sessions and reset daily.";
        let r = parse_gemini_status(pane).expect("gemini quota");
        let day = win(&r, "day");
        assert_eq!(day.pct_used, 37);
        assert!(day.reset_at.is_some());
    }

    #[test]
    fn parses_gemini_limit_reached() {
        let r = parse_gemini_status("Limit reached, resets in 45m").expect("exhausted");
        assert_eq!(win(&r, "day").pct_used, 100);
    }

    #[test]
    fn trust_prompt_yields_none() {
        let pane = include_str!("fixtures/trust_prompt.txt");
        assert!(parse_usage(pane).is_none());
        assert!(parse_codex_status(pane).is_none());
        assert!(parse_gemini_status(pane).is_none());
    }

    #[test]
    fn blank_and_ordinary_output_yield_none() {
        assert!(parse_usage("").is_none());
        assert!(parse_usage("running 24 tests\ntest result: ok").is_none());
        assert!(parse_codex_status("just some\noutput lines").is_none());
        // Gemini's per-session token stats (no account quota) must not parse as usage.
        assert!(parse_gemini_status("Session Stats\nTokens: 1234 input, 567 output").is_none());
    }

    #[test]
    fn claude_partial_screen_session_only() {
        let pane = "Current session\n  ████ 22% used\n  Resets 3:00pm\n";
        let r = parse_usage(pane).expect("partial parse");
        assert_eq!(r.windows.len(), 1);
        assert_eq!(win(&r, "5h").pct_used, 22);
    }

    #[test]
    fn claude_sections_do_not_borrow_each_others_numbers() {
        // No percentage under "Current session" → no 5h window; it must not grab the week's 88%.
        let pane = "Current session\n  Resets 3:00pm\n\nCurrent week (all models)\n  88% used\n";
        let r = parse_usage(pane).expect("week parsed");
        assert!(r.windows.iter().all(|w| w.label != "5h"));
        assert_eq!(win(&r, "wk").pct_used, 88);
    }

    #[test]
    fn codex_paid_style_5h_and_weekly() {
        // Inferred paid layout (only the Free capture is a real fixture): 5h + weekly windows.
        let pane = "  5h limit:      [██░] 32% left (resets 14:00)\n  \
                    Weekly limit:  [█░] 88% left (resets 09:00 on 21 Jun)\n";
        let r = parse_codex_status(pane).expect("paid parse");
        assert_eq!(win(&r, "5h").pct_used, 68); // 100 - 32
        assert_eq!(win(&r, "wk").pct_used, 12); // 100 - 88
    }
}
