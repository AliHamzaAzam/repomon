//! Detecting Claude Code's usage-limit pause from pane text, and parsing the reset time.
//!
//! Claude's transcript JSONL does **not** record usage-limit info, so the daemon detects the
//! pause by reading the agent's tmux pane. This module is the pure, fixture-tested heart: given
//! the recent pane text it decides whether the agent is *blocked* on the usage limit and, if so,
//! when it resets. It is deliberately lenient about phrasing and never matches the non-blocking
//! "approaching usage limit" warning.

use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone, Utc};

/// A parsed reset time this far (or less) in the past is treated as "just reset, resume now"
/// rather than rolled to tomorrow — Claude's resets are always within a few hours, so a time well
/// beyond this is a genuine next-day (cross-midnight) reset.
const GRACE_PAST_HOURS: i64 = 6;

/// A detected usage-limit pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimit {
    /// When the limit resets (UTC), if a clock time could be parsed from the message. `None`
    /// means the caller should retry periodically rather than wait for a precise moment.
    pub reset_at: Option<DateTime<Utc>>,
    /// Claude's interactive "What do you want to do?" menu, parsed from the pane when on
    /// screen. The caller must select the "stop and wait for limit to reset" option — which is
    /// NOT always option 1 nor always pre-selected (the options move around between
    /// occurrences) — see [`menu_select_keys`].
    pub menu: Option<LimitMenu>,
}

/// The interactive usage-limit menu as read from the pane: where the cursor is and where the
/// "stop and wait" option actually sits, so the caller selects by position, not by faith.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitMenu {
    /// 0-based row of the selection cursor (`❯`), if visible.
    pub selected: Option<usize>,
    /// 0-based row of the "stop and wait for limit to reset" option.
    pub wait_idx: usize,
    /// The number printed beside the wait option ("2. Stop and wait…" → 2), for the
    /// no-visible-cursor fallback.
    pub wait_number: Option<u32>,
}

/// Detect the **blocking** usage-limit state in an agent's recent pane text. Returns `None` for
/// ordinary output and for the non-blocking "approaching usage limit" warning.
pub fn detect_usage_limit(pane: &str) -> Option<UsageLimit> {
    let lower = pane.to_lowercase();
    // The newer flow shows an interactive menu offering "Stop and wait for limit to reset"; it
    // carries none of the classic "limit reached" phrasing, so detect it explicitly.
    let menu = parse_menu(pane);
    if !is_blocked(&lower) && menu.is_none() {
        return None;
    }
    Some(UsageLimit {
        reset_at: parse_reset_at(&lower, Local::now()),
        menu,
    })
}

/// Whether a stripped option text is the "stop and wait" choice.
fn is_wait_option(lower_text: &str) -> bool {
    lower_text.contains("stop and wait") || lower_text.contains("wait for limit")
}

/// Parse one menu-option-shaped line: optional `❯` cursor, optional `N.` number, then text.
/// Returns `(has_cursor, number, text)`; `None` when the line isn't option-shaped.
/// (Shared with the pending-prompt detector in [`super::prompt`].)
pub(crate) fn parse_option_line(line: &str) -> Option<(bool, Option<u32>, String)> {
    let clean = strip_ansi(line);
    let mut rest = clean.trim_start();
    let cursor = rest.starts_with('❯');
    if cursor {
        rest = rest.trim_start_matches('❯').trim_start();
    }
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .take(2)
        .collect();
    let number = if !digits.is_empty() && rest[digits.len()..].starts_with('.') {
        rest = rest[digits.len() + 1..].trim_start();
        digits.parse::<u32>().ok()
    } else {
        None
    };
    // An option row needs at least one of the markers (cursor or number) plus some text —
    // otherwise every ordinary output line would qualify.
    if (!cursor && number.is_none()) || rest.is_empty() {
        return None;
    }
    Some((cursor, number, rest.to_string()))
}

/// Read Claude's usage-limit menu from the pane, anchored on the "stop and wait" option so
/// numbered lists in ordinary agent output never parse as a menu. The menu block is the run of
/// contiguous option-shaped lines around that anchor.
fn parse_menu(pane: &str) -> Option<LimitMenu> {
    let lines: Vec<&str> = pane.lines().collect();
    let parsed: Vec<Option<(bool, Option<u32>, String)>> =
        lines.iter().map(|l| parse_option_line(l)).collect();
    let anchor = parsed.iter().position(|p| {
        p.as_ref()
            .is_some_and(|(_, _, text)| is_wait_option(&text.to_lowercase()))
    })?;
    // Expand to the contiguous option block around the anchor.
    let mut start = anchor;
    while start > 0 && parsed[start - 1].is_some() {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < parsed.len() && parsed[end].is_some() {
        end += 1;
    }
    let block: Vec<&(bool, Option<u32>, String)> = parsed[start..end].iter().flatten().collect();
    let wait_idx = anchor - start;
    Some(LimitMenu {
        selected: block.iter().position(|(cursor, _, _)| *cursor),
        wait_idx,
        wait_number: block[wait_idx].1,
    })
}

/// The keystrokes (tmux `send-keys` names) that select the menu's wait option: arrow from the
/// visible cursor to the option's row, then Enter. Without a visible cursor, fall back to the
/// option's printed number (digit selection confirms immediately; the trailing Enter then lands
/// harmlessly on the empty input box).
pub fn menu_select_keys(menu: &LimitMenu) -> Vec<String> {
    match menu.selected {
        Some(cur) => {
            let (from, to) = (cur as i64, menu.wait_idx as i64);
            let arrow = if to > from { "Down" } else { "Up" };
            let mut keys = vec![arrow.to_string(); (to - from).unsigned_abs() as usize];
            keys.push("Enter".into());
            keys
        }
        None => match menu.wait_number {
            Some(n) => vec![n.to_string(), "Enter".into()],
            None => vec!["Enter".into()],
        },
    }
}

/// Drop ANSI CSI/OSC escape sequences (pane captures use `-e`, so menu rows carry color and
/// inverse-video escapes that would break per-line parsing).
pub(crate) fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ … final byte in @-~
            Some('[') => {
                chars.next();
                for n in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&n) {
                        break;
                    }
                }
            }
            // OSC: ESC ] … BEL (or ESC \)
            Some(']') => {
                chars.next();
                while let Some(n) = chars.next() {
                    if n == '\u{07}' {
                        break;
                    }
                    if n == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether the pane shows a *blocking* limit. Covers Claude's several phrasings: the classic
/// "usage limit reached … resets at X", the "You've hit your session limit · resets 3am" notice,
/// and any screen offering "/upgrade to increase your usage limit". The "approaching … limit"
/// heads-up is a warning, not a block, so it's explicitly excluded.
fn is_blocked(lower: &str) -> bool {
    if lower.contains("approaching") {
        return false;
    }
    // A strong, Claude-specific signal that stands on its own.
    if lower.contains("upgrade to increase your usage limit") {
        return true;
    }
    let reached = lower.contains("usage limit reached")
        || lower.contains("reached your usage limit")
        || lower.contains("limit reached")
        || lower.contains("hit your usage limit")
        || lower.contains("hit your session limit");
    let reset_cue = lower.contains("reset") || lower.contains("try again");
    reached && reset_cue
}

/// Claude always states the *next* reset, which is at most a few hours out, formatted in the
/// machine's local timezone. So today's occurrence is the answer when it's upcoming **or only
/// recently passed** (within [`GRACE_PAST_HOURS`]) — the latter means the limit just reset and a
/// stuck agent we notice a moment late should resume now, not wait a day. Only a time that's
/// *well* in the past (a cross-midnight "resets 3am" seen at night) rolls to tomorrow. Returns
/// `None` if no clock time is present.
fn parse_reset_at(lower: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    let time = find_reset_time(lower)?;
    let date = now.date_naive();
    let naive = date.and_time(time);
    let mut dt = Local.from_local_datetime(&naive).earliest()?;
    if now - dt > Duration::hours(GRACE_PAST_HOURS) {
        let naive2 = (date + Duration::days(1)).and_time(time);
        dt = Local.from_local_datetime(&naive2).earliest()?;
    }
    Some(dt.with_timezone(&Utc))
}

/// Find the reset clock time — preferring one that appears after a "reset"/"again" cue, falling
/// back to the first time anywhere in the text.
fn find_reset_time(lower: &str) -> Option<NaiveTime> {
    let cue = ["reset", "again"]
        .iter()
        .filter_map(|c| lower.find(c))
        .min();
    if let Some(idx) = cue {
        if let Some(t) = parse_first_time(&lower[idx..]) {
            return Some(t);
        }
    }
    parse_first_time(lower)
}

/// Scan for the first clock time. A bare integer is **not** a time (so "5-hour limit" and stray
/// numbers don't match) — a match needs a `:mm` minute or an `am`/`pm` marker.
fn parse_first_time(s: &str) -> Option<NaiveTime> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let starts_number =
            b[i].is_ascii_digit() && (i == 0 || !(b[i - 1].is_ascii_digit() || b[i - 1] == b':'));
        if starts_number {
            if let Some(t) = try_parse_time_at(b, i) {
                return Some(t);
            }
        }
        i += 1;
    }
    None
}

/// Try to parse `H`, `H:MM`, `H am/pm`, or `H:MM am/pm` (also 24-hour `HH:MM`) starting at `start`.
fn try_parse_time_at(b: &[u8], start: usize) -> Option<NaiveTime> {
    let mut i = start;
    let h0 = i;
    while i < b.len() && b[i].is_ascii_digit() && i - h0 < 2 {
        i += 1;
    }
    let hour: u32 = std::str::from_utf8(&b[h0..i]).ok()?.parse().ok()?;

    let mut minute: u32 = 0;
    let mut had_minute = false;
    if i < b.len() && b[i] == b':' {
        let m0 = i + 1;
        let mut j = m0;
        while j < b.len() && b[j].is_ascii_digit() && j - m0 < 2 {
            j += 1;
        }
        if j - m0 != 2 {
            return None; // "3:" with no two-digit minute isn't a time
        }
        minute = std::str::from_utf8(&b[m0..j]).ok()?.parse().ok()?;
        had_minute = true;
        i = j;
    }

    // Optional spaces, then an am/pm marker — allowing periods (am, a.m., pm, p.m.).
    let mut k = i;
    while k < b.len() && b[k] == b' ' {
        k += 1;
    }
    let pm = match b.get(k) {
        Some(m @ (b'a' | b'p')) => {
            let mut p = k + 1;
            if b.get(p) == Some(&b'.') {
                p += 1; // the '.' in "a.m."
            }
            if b.get(p) == Some(&b'm') {
                Some(*m == b'p')
            } else {
                None
            }
        }
        _ => None,
    };

    // Require a minute or an am/pm marker so bare integers (e.g. "5-hour") don't match.
    if !had_minute && pm.is_none() {
        return None;
    }
    if minute > 59 {
        return None;
    }
    let hour24 = match pm {
        Some(true) if hour == 12 => 12,
        Some(true) if hour <= 11 => hour + 12,
        Some(false) if hour == 12 => 0,
        Some(false) if hour <= 11 => hour,
        None if hour <= 23 => hour,
        _ => return None,
    };
    NaiveTime::from_hms_opt(hour24, minute, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn detects_blocking_message_with_time() {
        let pane = "● Working...\nClaude usage limit reached. Your limit will reset at 3:00 PM.";
        let lim = detect_usage_limit(pane).expect("should detect a block");
        assert!(lim.reset_at.is_some(), "should parse a reset time");
    }

    #[test]
    fn detects_try_again_24h() {
        let pane = "You've reached your usage limit. Please try again at 15:30.";
        let lim = detect_usage_limit(pane).expect("block");
        let local = lim.reset_at.unwrap().with_timezone(&Local);
        assert_eq!((local.hour(), local.minute()), (15, 30));
    }

    #[test]
    fn ignores_approaching_warning() {
        let pane = "Approaching usage limit — your limit resets at 5pm. Keep going.";
        assert!(detect_usage_limit(pane).is_none());
    }

    #[test]
    fn ignores_ordinary_output() {
        let pane = "running 24 tests\ntest result: ok. 24 passed; 0 failed in 1.2s";
        assert!(detect_usage_limit(pane).is_none());
    }

    #[test]
    fn block_without_time_has_none_reset() {
        let pane = "Claude usage limit reached. Please try again later.";
        let lim = detect_usage_limit(pane).expect("block");
        assert_eq!(lim.reset_at, None);
        assert!(
            lim.menu.is_none(),
            "the classic message is not the interactive menu"
        );
    }

    #[test]
    fn detects_interactive_menu() {
        // The newer flow: an interactive menu with no "limit reached" phrasing and no time.
        let pane = "What do you want to do?\n\
            ❯ 1. Stop and wait for limit to reset\n\
              2. Upgrade your plan\n\
              3. Upgrade to Team plan\n\
            Enter to confirm · Esc to cancel";
        let lim = detect_usage_limit(pane).expect("should detect the menu");
        let menu = lim.menu.expect("menu should be parsed");
        assert_eq!(menu.selected, Some(0), "cursor on the first row");
        assert_eq!(menu.wait_idx, 0);
        assert_eq!(menu.wait_number, Some(1));
        assert_eq!(lim.reset_at, None, "no time shown → retry periodically");
    }

    #[test]
    fn menu_with_reset_time_parses_it() {
        // If the menu (or the screen after picking option 1) shows a reset time, capture it.
        let pane = "Your limit will reset at 3:00 PM.\n\
            What do you want to do?\n\
            ❯ 1. Stop and wait for limit to reset\n\
              2. Upgrade your plan";
        let lim = detect_usage_limit(pane).expect("menu");
        assert!(lim.menu.is_some());
        let local = lim.reset_at.expect("3pm").with_timezone(&Local);
        assert_eq!(local.hour(), 15);
    }

    #[test]
    fn menu_parses_reordered_options() {
        // The options move around between occurrences — the wait choice here is option 2 and
        // the cursor sits on option 1. A blind Enter would pick "Upgrade your plan".
        let pane = "What do you want to do?\n\
            ❯ 1. Upgrade your plan\n\
              2. Stop and wait for limit to reset\n\
              3. Upgrade to Team plan\n\
            Enter to confirm · Esc to cancel";
        let menu = detect_usage_limit(pane)
            .expect("menu")
            .menu
            .expect("parsed");
        assert_eq!(menu.selected, Some(0));
        assert_eq!(menu.wait_idx, 1);
        assert_eq!(menu.wait_number, Some(2));
        assert_eq!(menu_select_keys(&menu), vec!["Down", "Enter"]);
    }

    #[test]
    fn menu_parsing_strips_ansi() {
        // Pane captures use `-e`, so rows carry color/inverse escapes.
        let pane = "What do you want to do?\n\
            \u{1b}[7m❯ 1. Upgrade your plan\u{1b}[0m\n\
            \u{1b}[2m  2. \u{1b}[1mStop and wait\u{1b}[0m\u{1b}[2m for limit to reset\u{1b}[0m";
        let menu = detect_usage_limit(pane)
            .expect("menu")
            .menu
            .expect("parsed despite ANSI");
        assert_eq!(menu.selected, Some(0));
        assert_eq!(menu.wait_idx, 1);
        assert_eq!(menu.wait_number, Some(2));
    }

    #[test]
    fn numbered_output_is_not_a_menu() {
        // Ordinary agent output with a numbered list must not parse as a limit menu.
        let pane = "Here's the plan:\n\
            1. Refactor the parser\n\
            2. Add tests\n\
            3. Ship it";
        assert!(parse_menu(pane).is_none());
        assert!(detect_usage_limit(pane).is_none());
    }

    #[test]
    fn menu_select_keys_paths() {
        let menu = |selected, wait_idx, wait_number| LimitMenu {
            selected,
            wait_idx,
            wait_number,
        };
        // Cursor already on the wait option → just confirm (the old behavior, now verified).
        assert_eq!(menu_select_keys(&menu(Some(0), 0, Some(1))), vec!["Enter"]);
        // Below the cursor → walk down.
        assert_eq!(
            menu_select_keys(&menu(Some(0), 2, Some(3))),
            vec!["Down", "Down", "Enter"]
        );
        // Above the cursor → walk up.
        assert_eq!(
            menu_select_keys(&menu(Some(2), 0, Some(1))),
            vec!["Up", "Up", "Enter"]
        );
        // No visible cursor → select by printed number.
        assert_eq!(
            menu_select_keys(&menu(None, 1, Some(2))),
            vec!["2", "Enter"]
        );
        // No cursor and no number: Enter is the only signal left.
        assert_eq!(menu_select_keys(&menu(None, 0, None)), vec!["Enter"]);
    }

    #[test]
    fn five_hour_phrase_does_not_parse_as_time() {
        // "5-hour" must not be read as 05:00; the real time is "9:30 am".
        let pane = "Your 5-hour usage limit reached. Resets at 9:30 am.";
        let lim = detect_usage_limit(pane).expect("block");
        let local = lim.reset_at.expect("9:30 am").with_timezone(&Local);
        assert_eq!((local.hour(), local.minute()), (9, 30));
    }

    #[test]
    fn rolls_to_tomorrow_only_when_well_past() {
        let now = Local.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap(); // 6pm

        // Just passed (3h ago, within grace) → today, so a stuck agent resumes promptly.
        let at_3pm = parse_reset_at("resets at 3:00 pm", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(at_3pm.day(), 1); // today
        assert_eq!(at_3pm.hour(), 15);

        // Well in the past (cross-midnight "3am" seen at 6pm = 15h) → tomorrow.
        let at_3am = parse_reset_at("resets at 3am", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(at_3am.day(), 2); // tomorrow
        assert_eq!(at_3am.hour(), 3);

        // Still upcoming today.
        let at_11pm = parse_reset_at("resets at 11:00 pm", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(at_11pm.day(), 1); // still today
        assert_eq!(at_11pm.hour(), 23);
    }

    #[test]
    fn detects_session_limit_notice() {
        // Claude's "session limit" phrasing — no "limit reached", no menu, but a reset time and
        // an upgrade cue. (The reported real-world miss.)
        let pane = "You've hit your session limit · resets 3am (Asia/Karachi)\n\
            /upgrade to increase your usage limit.";
        let lim = detect_usage_limit(pane).expect("session limit is a block");
        assert!(
            lim.menu.is_none(),
            "this notice is not the interactive menu"
        );
        let local = lim.reset_at.expect("3am").with_timezone(&Local);
        assert_eq!(local.hour(), 3);
    }

    #[test]
    fn upgrade_cue_alone_is_a_block() {
        let pane = "/upgrade to increase your usage limit.";
        assert!(detect_usage_limit(pane).is_some());
    }

    #[test]
    fn parses_pm_without_minutes() {
        let now = Local.with_ymd_and_hms(2026, 6, 1, 8, 0, 0).unwrap();
        let t = parse_reset_at("try again at 3pm", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(t.hour(), 15);
    }

    #[test]
    fn parses_meridiem_with_periods() {
        // Some locales render "p.m."; treat it the same as "pm".
        let now = Local.with_ymd_and_hms(2026, 6, 1, 8, 0, 0).unwrap();
        let t = parse_reset_at("resets at 4:34 p.m.", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!((t.hour(), t.minute()), (16, 34));
    }
}
