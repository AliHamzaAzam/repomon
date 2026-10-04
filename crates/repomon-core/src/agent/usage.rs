//! Parses usage screens by labels rather than fixed positions and normalizes percentages to used
//! capacity. Unrecognized layouts return no snapshot.

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use super::text::{parse_pct, parse_reset_datetime, strip_ansi};

/// One usage limit window, normalized across agents. `pct_used` is how much of the window is
/// consumed (0–100); `label` is a short tag for display (`5h`, `wk`, `mo`, or a model name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageWindow {
    pub label: String,
    pub pct_used: u8,
    pub reset_at: Option<DateTime<Utc>>,
}

/// An account's usage: an ordered list of limit windows (shortest first). Empty/absent windows
/// mean nothing was readable - clients show nothing rather than zeros.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct AccountUsage {
    pub key: String,
    pub label: String,
    pub report: UsageReport,
    /// How long ago the probe captured this, in seconds.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub age_secs: u64,
}

/// Reports manual refresh outcomes, using Cooldown for a concurrent in-flight probe rather than the
/// ordinary freshness interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum UsageRefreshReason {
    /// A manual round was queued; completion arrives on `event.usage.refreshed`.
    Pending,
    /// At least one account's usage was freshly probed.
    Ok,
    /// `[usage_probe]` is off in Settings.
    ProbeDisabled,
    /// No agent kind the probe knows how to read is currently running.
    NoActiveKind,
    /// A refresh was already in flight; this request did not start another probe round.
    Cooldown,
    /// The round is still running after the notification deadline.
    Timeout,
    /// Every eligible account's probe failed.
    Error,
}

/// The outcome of a manually-triggered usage refresh, returned by `usage.refresh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageRefreshResult {
    /// Whether this call's probe round actually refreshed at least one account.
    pub refreshed: bool,
    /// Correlates a pending response with its completion event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub request_id: Option<u64>,
    pub reason: UsageRefreshReason,
    /// A short, human-readable elaboration, set for the non-`ok` reasons that benefit from one.
    pub detail: Option<String>,
    /// The usage snapshot after the round, same shape as `usage.get`. Present even when
    /// `refreshed` is false, so a client can always re-render from the response.
    pub snapshot: Vec<AccountUsage>,
}

/// A manual probe notification. Detail distinguishes a still-running round from a final timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
#[serde(rename_all = "snake_case")]
pub enum UsageRefreshedReason {
    Ok,
    ProbeDisabled,
    NoActiveKind,
    Timeout,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageRefreshed {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub request_id: u64,
    pub reason: UsageRefreshedReason,
    pub detail: Option<String>,
    pub snapshot: Vec<AccountUsage>,
}

/// Parse Claude's `/usage` screen. Sections: "Current session" (the 5-hour window), "Current week
/// (all models)", and a model-specific weekly ("(Opus)"/"(Sonnet only)"). Returns `None` when no
/// percentage is found in the newest screen (a blank/loading/trust screen yields no fake zeros).
pub fn parse_usage(pane: &str) -> Option<UsageReport> {
    let lines: Vec<String> = pane.lines().map(strip_ansi).collect();
    let start = lines
        .iter()
        .rposition(|line| {
            let low = screen_text(line).to_ascii_lowercase();
            low == "/usage"
                || low.starts_with("current session")
                || (low.starts_with("settings ")
                    && low.split_whitespace().any(|word| word == "usage"))
        })
        .unwrap_or(0);
    let lines = &lines[start..];
    let now = Local::now();
    let mut windows = Vec::new();
    let mut in_limits = false;

    for (i, line) in lines.iter().enumerate() {
        let low = screen_text(line).to_ascii_lowercase();
        if low.starts_with('❯') || low.starts_with('╰') {
            break;
        }
        if low.starts_with("current session") {
            in_limits = true;
            if let Some((pct, reset)) = section_after(lines, i, now) {
                windows.push(window("5h", pct, reset));
            }
        } else if low.starts_with("current week") {
            in_limits = true;
            let label = if low.contains("all models") {
                "wk".to_string()
            } else {
                week_model_label(&low)
            };
            if let Some((pct, reset)) = section_after(lines, i, now) {
                windows.push(window(&label, pct, reset));
            }
        } else if in_limits
            && !low.is_empty()
            && !low.contains("% used")
            && !low.starts_with("resets ")
        {
            break;
        }
    }

    windows.sort_by_key(|w| w.label != "5h");
    (!windows.is_empty()).then_some(UsageReport { windows })
}

/// Parse Codex's `/status` screen. The lines look like
/// `│  Monthly limit:  [bars] 95% left (resets 04:00 on 19 Jul) │` - note Codex reports **% left**
/// (converted to % used here) and may show 5-hour, weekly, or (Free) monthly windows.
pub fn parse_codex_status(pane: &str) -> Option<UsageReport> {
    let lines: Vec<String> = pane.lines().map(strip_ansi).collect();
    // Retries append status blocks to the visible pane; even an unfinished newer block supersedes
    // the old one so the probe can wait for fresh limits instead of reporting stale values.
    let start = lines
        .iter()
        .rposition(|line| {
            let low = screen_text(line).to_ascii_lowercase();
            low == "/status" || low.starts_with(">_ openai codex")
        })
        .map_or(0, |i| i + 1);
    let now = Local::now();
    let mut windows = Vec::new();
    let mut in_limits = false;

    for line in &lines[start..] {
        let clean = screen_text(line);
        if clean.starts_with(['╰', '›', '/']) {
            break;
        }
        let low = clean.to_ascii_lowercase();
        // Anchor on "<name> limit:"; the "rate limits and credits" hint has no colon and is skipped.
        let Some(lpos) = low.find("limit:") else {
            if in_limits {
                break;
            }
            continue;
        };
        in_limits = true;
        let value = &low[lpos + "limit:".len()..];
        if !value.contains("% left") {
            continue;
        }
        let Some(left) = parse_pct(value) else {
            continue;
        };
        let pct_used = 100u8.saturating_sub(left);
        let reset = if low.contains("reset") {
            parse_reset_datetime(&low, now)
        } else {
            None
        };
        let (order, label) = codex_label(&clean[..lpos]);
        windows.push((order, window(&label, pct_used, reset)));
    }

    windows.sort_by_key(|(order, _)| *order);
    let windows: Vec<_> = windows.into_iter().map(|(_, w)| w).collect();
    (!windows.is_empty()).then_some(UsageReport { windows })
}

/// Parses Antigravity model-group limits, converting remaining percentages to used capacity and
/// resolving relative reset times.
pub fn parse_antigravity_usage(pane: &str) -> Option<UsageReport> {
    let now = Local::now();
    let mut windows = Vec::new();
    let lines: Vec<String> = pane.lines().map(strip_ansi).collect();

    let mut current_prefix = "";

    for (i, line) in lines.iter().enumerate() {
        let low = line.to_lowercase();
        if low.contains("gemini models") {
            current_prefix = "";
        } else if low.contains("claude") && low.contains("models") {
            current_prefix = "claude-";
        } else if low.contains("gpt") && low.contains("models") {
            current_prefix = "gpt-";
        }

        let is_5h = (low.contains("five hour") || low.contains("5-hour") || low.contains("5 hour"))
            && low.contains("limit");
        let is_wk = low.contains("weekly") && low.contains("limit");

        if is_5h || is_wk {
            let label = if is_5h {
                format!("{current_prefix}5h")
            } else {
                format!("{current_prefix}wk")
            };

            let mut pct_left = None;
            let mut reset = None;
            for next_line in lines.iter().skip(i + 1).take(4) {
                let next_low = next_line.to_lowercase();
                if (next_low.contains("limit remaining") || next_low.contains("models"))
                    && !next_low.contains("within this group")
                {
                    break;
                }
                if pct_left.is_none() {
                    pct_left = parse_pct(next_line);
                }
                if reset.is_none() && next_low.contains("refresh") {
                    reset = parse_reset_datetime(&next_low, now);
                }
            }

            if let Some(left) = pct_left {
                let pct_used = 100u8.saturating_sub(left);
                windows.push(window(&label, pct_used, reset));
            }
        }
    }

    if windows.is_empty() {
        return None;
    }

    windows.sort_by_key(|w| match w.label.as_str() {
        "5h" => 0,
        "wk" => 1,
        "claude-5h" => 2,
        "claude-wk" => 3,
        _ => 4,
    });

    Some(UsageReport { windows })
}

fn window(label: &str, pct_used: u8, reset_at: Option<DateTime<Utc>>) -> UsageWindow {
    UsageWindow {
        label: label.to_string(),
        pct_used,
        reset_at,
    }
}

fn screen_text(line: &str) -> &str {
    line.trim().trim_matches('│').trim()
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
        let low = screen_text(line).to_ascii_lowercase();
        if low.contains("% used") {
            if pct.is_none() {
                pct = parse_pct(line);
            }
        } else if low.starts_with("resets ") {
            if reset.is_none() {
                reset = parse_reset_datetime(&low, now);
            }
        } else if !low.is_empty() {
            break;
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

/// Duration order and display label from a Codex limit's name, preserving weekly quota scopes.
fn codex_label(before_limit: &str) -> (u8, String) {
    let n = before_limit.to_ascii_lowercase();
    if n.contains("5h") || n.contains("hour") {
        (0, "5h".to_string())
    } else if n.contains("week") {
        let scope = before_limit
            .split_whitespace()
            .filter(|word| {
                !word.eq_ignore_ascii_case("weekly") && !word.eq_ignore_ascii_case("week")
            })
            .collect::<Vec<_>>()
            .join(" ");
        (2, if scope.is_empty() { "wk".into() } else { scope })
    } else if n.contains("month") {
        (3, "mo".to_string())
    } else if n.contains("day") || n.contains("daily") {
        (1, "day".to_string())
    } else {
        // Fall back to the last alphanumeric word before "limit:".
        let label = n
            .split(|c: char| !c.is_alphanumeric())
            .rfind(|w| !w.is_empty())
            .unwrap_or("lim")
            .to_string();
        (4, label)
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
        assert_eq!(win(&r, "sonnet").pct_used, 0);

        let s = win(&r, "5h").reset_at.unwrap().with_timezone(&Local);
        assert_eq!((s.hour(), s.minute()), (23, 59));
        let w = win(&r, "wk").reset_at.unwrap().with_timezone(&Local);
        assert_eq!((w.month(), w.day()), (6, 21));
        assert_eq!((w.hour(), w.minute()), (19, 59));
    }

    #[test]
    fn parses_real_codex_status_capture() {
        // The real `/status` screen from Codex CLI v0.141.0 (Free plan: a monthly window).
        let pane = include_str!("fixtures/codex_status_v0.ansi");
        let r = parse_codex_status(pane).expect("should parse the /status screen");
        let mo = win(&r, "mo");
        assert_eq!(mo.pct_used, 5);
        let reset = mo.reset_at.expect("monthly reset").with_timezone(&Local);
        assert_eq!((reset.month(), reset.day()), (7, 19));
        assert_eq!((reset.hour(), reset.minute()), (4, 0));
    }

    #[test]
    fn codex_repeated_pro_status_uses_latest_block() {
        let pane = include_str!("fixtures/codex_status_pro_v0.160_repeated.ansi");
        let r = parse_codex_status(pane).expect("Pro status parsed");
        assert_eq!(
            r.windows.len(),
            2,
            "one pair of limits from the latest status"
        );
        assert_eq!(r.windows[0].pct_used, 70);
        assert_eq!(r.windows[1].pct_used, 82);
    }

    #[test]
    fn codex_pro_weekly_scopes_are_distinct() {
        let pane = include_str!("fixtures/codex_status_pro_v0.160.ansi");
        let r = parse_codex_status(pane).expect("Pro status parsed");
        assert_eq!(r.windows.len(), 2);
        assert_eq!(win(&r, "wk").pct_used, 68);
        assert_eq!(win(&r, "Luna Reserve").pct_used, 82);
        let reset = win(&r, "Luna Reserve")
            .reset_at
            .expect("reserve reset")
            .with_timezone(&Local);
        assert_eq!((reset.month(), reset.day()), (10, 3));
        assert_eq!((reset.hour(), reset.minute()), (18, 14));
    }

    #[test]
    fn claude_repeated_usage_uses_latest_block() {
        let pane = include_str!("fixtures/usage_v2.ansi");
        let newer = pane.replace("15% used", "22% used");
        let r = parse_usage(&format!("{pane}\n{newer}")).expect("latest usage parsed");
        assert_eq!(r.windows.len(), 3);
        assert_eq!(win(&r, "5h").pct_used, 22);
        assert_eq!(win(&r, "wk").pct_used, 41);
        assert_eq!(win(&r, "sonnet").pct_used, 0);
    }

    #[test]
    fn codex_loading_status_does_not_reuse_previous_block() {
        let pane = include_str!("fixtures/codex_status_pro_v0.160.ansi");
        for latest in ["/status\n", ">_ OpenAI Codex (v0.160.0)\nLoading limits\n"] {
            assert!(parse_codex_status(&format!("{pane}\n{latest}")).is_none());
        }
    }

    #[test]
    fn codex_limit_group_ends_before_unrelated_output() {
        let pane = ">_ OpenAI Codex (v0.160.0)\nWeekly limit: 30% left\n\n\
                    Example output:\nWeekly limit: 1% left\n";
        let r = parse_codex_status(pane).expect("status parsed");
        assert_eq!(r.windows, vec![window("wk", 70, None)]);

        let pane = include_str!("fixtures/codex_status_v0.ansi");
        let r = parse_codex_status(&format!("{pane}\nWeekly limit: 1% left\n"))
            .expect("boxed status parsed");
        assert_eq!(r.windows.len(), 1);
        assert_eq!(r.windows[0].label, "mo");
    }

    #[test]
    fn codex_preserves_repeated_windows_within_one_block() {
        let pane = "/status\n>_ OpenAI Codex (v0.160.0)\n\
                    Weekly limit: 30% left\nWeekly limit: 30% left\n";
        let r = parse_codex_status(pane).expect("status parsed");
        assert_eq!(r.windows, vec![window("wk", 70, None); 2]);
    }

    #[test]
    fn codex_windows_are_shortest_first_with_stable_weekly_scopes() {
        let pane = "/status\n>_ OpenAI Codex (v0.160.0)\n\
                    Monthly limit: 90% left\nLuna Reserve Weekly limit: 70% left\n\
                    Daily limit: 80% left\nWeekly limit: 60% left\n5h limit: 50% left\n";
        let r = parse_codex_status(pane).expect("status parsed");
        assert_eq!(
            r.windows,
            vec![
                window("5h", 50, None),
                window("day", 20, None),
                window("Luna Reserve", 30, None),
                window("wk", 40, None),
                window("mo", 10, None),
            ]
        );
    }

    #[test]
    fn claude_loading_usage_does_not_reuse_previous_block() {
        let pane = include_str!("fixtures/usage_v2.ansi");
        for latest in [
            "/usage\n",
            "Settings Status Config Usage Stats\nLoading usage\n",
        ] {
            assert!(parse_usage(&format!("{pane}\n{latest}")).is_none());
        }
    }

    #[test]
    fn claude_repeated_partial_usage_uses_latest_session() {
        let pane = "Current session\n15% used\nCurrent week (all models)\n41% used\n\
                    Current session\n22% used\nCurrent week (all models)\n51% used\n";
        let r = parse_usage(pane).expect("latest partial usage parsed");
        assert_eq!(
            r.windows,
            vec![window("5h", 22, None), window("wk", 51, None)]
        );
    }

    #[test]
    fn claude_limits_do_not_borrow_diagnostic_percentages() {
        let pane = "Current session\n22% used\nCurrent week (all models)\n\n\
                    What's contributing to your limits usage?\n78% of your usage came from long sessions\n\
                    Current week (Sonnet only)\n12% used\n";
        let r = parse_usage(pane).expect("session parsed");
        assert_eq!(r.windows, vec![window("5h", 22, None)]);
    }

    #[test]
    fn trust_prompt_yields_none() {
        let pane = include_str!("fixtures/trust_prompt.txt");
        assert!(parse_usage(pane).is_none());
        assert!(parse_codex_status(pane).is_none());
    }

    #[test]
    fn blank_and_ordinary_output_yield_none() {
        assert!(parse_usage("").is_none());
        assert!(parse_usage("running 24 tests\ntest result: ok").is_none());
        assert!(parse_codex_status("just some\noutput lines").is_none());
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
    fn codex_synthetic_5h_and_weekly() {
        // Synthetic layout; the real Pro 100 captures have only weekly windows.
        let pane = "  5h limit:      [██░] 32% left (resets 14:00)\n  \
                    Weekly limit:  [█░] 88% left (resets 09:00 on 21 Jun)\n";
        let r = parse_codex_status(pane).expect("paid parse");
        assert_eq!(win(&r, "5h").pct_used, 68);
        assert_eq!(win(&r, "wk").pct_used, 12);
    }

    #[test]
    fn parses_antigravity_models_quota_capture() {
        let pane = r#"
└ Models & Quota

  Account: user@example.com

GEMINI MODELS
  Models within this group: Gemini Flash, Gemini Pro

  Weekly Limit Remaining
    [███████████████████████████████████░░░░░░░░░░░░░░░] 70.80%
    71% remaining · Refreshes in 106h 14m

  Five Hour Limit Remaining
    [██████████████████████████████████████████████░░░░] 92.36%
    92% remaining · Refreshes in 4h 18m


CLAUDE AND GPT MODELS
  Models within this group: Claude Opus, Claude Sonnet, GPT-OSS

  Weekly Limit Remaining
    [██████████████████████████████████████████████████] 100.00%
    Quota available

  Five Hour Limit Remaining
    [██████████████████████████████████████████████████] 100.00%
    Quota available
"#;
        let r = parse_antigravity_usage(pane).expect("antigravity usage parsed");
        assert_eq!(win(&r, "5h").pct_used, 8);
        assert_eq!(win(&r, "wk").pct_used, 29);
        assert_eq!(win(&r, "claude-5h").pct_used, 0);
        assert_eq!(win(&r, "claude-wk").pct_used, 0);
        assert!(win(&r, "5h").reset_at.is_some());
        assert!(win(&r, "wk").reset_at.is_some());
    }
}
