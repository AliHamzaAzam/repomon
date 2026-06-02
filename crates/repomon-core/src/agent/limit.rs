//! Detecting Claude Code's usage-limit pause from pane text, and parsing the reset time.
//!
//! Claude's transcript JSONL does **not** record usage-limit info, so the daemon detects the
//! pause by reading the agent's tmux pane. This module is the pure, fixture-tested heart: given
//! the recent pane text it decides whether the agent is *blocked* on the usage limit and, if so,
//! when it resets. It is deliberately lenient about phrasing and never matches the non-blocking
//! "approaching usage limit" warning.

use chrono::{DateTime, Duration, Local, NaiveTime, TimeZone, Utc};

/// A detected usage-limit pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageLimit {
    /// When the limit resets (UTC), if a clock time could be parsed from the message. `None`
    /// means the caller should retry periodically rather than wait for a precise moment.
    pub reset_at: Option<DateTime<Utc>>,
    /// Claude's newer interactive "What do you want to do?" menu is on screen. The caller must
    /// pick option 1 ("Stop and wait for limit to reset") — a bare Enter on the default — before
    /// it can resume.
    pub menu: bool,
}

/// Detect the **blocking** usage-limit state in an agent's recent pane text. Returns `None` for
/// ordinary output and for the non-blocking "approaching usage limit" warning.
pub fn detect_usage_limit(pane: &str) -> Option<UsageLimit> {
    let lower = pane.to_lowercase();
    // The newer flow shows an interactive menu whose first option is "Stop and wait for limit to
    // reset"; it carries none of the classic "limit reached" phrasing, so detect it explicitly.
    let menu = lower.contains("stop and wait for limit to reset");
    if !is_blocked(&lower) && !menu {
        return None;
    }
    Some(UsageLimit {
        reset_at: parse_reset_at(&lower, Local::now()),
        menu,
    })
}

/// A real block always says the limit was *reached* (paired with a reset / try-again cue). The
/// "approaching usage limit" warning has no "reached" phrase, so it never trips this.
fn is_blocked(lower: &str) -> bool {
    let reached = lower.contains("usage limit reached")
        || lower.contains("reached your usage limit")
        || lower.contains("limit reached");
    let reset_cue = lower.contains("reset") || lower.contains("try again");
    reached && reset_cue
}

/// Parse the reset time from a (lowercased) limit message, relative to `now` in local time —
/// Claude formats the reset time in the machine's local timezone. Rolls to tomorrow if the time
/// has already passed today. Returns `None` if no clock time is present.
fn parse_reset_at(lower: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    let time = find_reset_time(lower)?;
    let date = now.date_naive();
    let naive = date.and_time(time);
    let mut dt = Local.from_local_datetime(&naive).earliest()?;
    if dt <= now {
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
        assert!(!lim.menu, "the classic message is not the interactive menu");
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
        assert!(lim.menu, "menu flag should be set");
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
        assert!(lim.menu);
        let local = lim.reset_at.expect("3pm").with_timezone(&Local);
        assert_eq!(local.hour(), 15);
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
    fn rolls_to_tomorrow_when_time_already_passed() {
        let now = Local.with_ymd_and_hms(2026, 6, 1, 18, 0, 0).unwrap(); // 6pm
        let at_3pm = parse_reset_at("resets at 3:00 pm", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(at_3pm.day(), 2); // tomorrow
        assert_eq!(at_3pm.hour(), 15);

        let at_11pm = parse_reset_at("resets at 11:00 pm", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(at_11pm.day(), 1); // still today
        assert_eq!(at_11pm.hour(), 23);
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
