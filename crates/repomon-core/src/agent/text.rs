//! Shared pane-text parsing: ANSI stripping, clock/date times, and percentages.
//!
//! These primitives are pure and fixture-tested through their callers. [`limit`](super::limit)
//! reads Claude's usage-limit *pause* from a pane; [`usage`](super::usage) reads the `/usage`
//! screen. Both need to strip the color escapes a `capture-pane -e` carries and to parse the
//! clock times Claude prints in the machine's local timezone — so that logic lives here once.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Utc};

/// A parsed reset time this far (or less) in the past is treated as "just reset" rather than
/// rolled forward — Claude's session resets are always within a few hours, so a time well beyond
/// this is a genuine next-day (cross-midnight) reset.
pub(crate) const GRACE_PAST_HOURS: i64 = 6;

/// Drop ANSI CSI/OSC escape sequences (pane captures use `-e`, so lines carry color and
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

/// Parse a `NN%` percentage: the integer immediately preceding a `%`. Returns the first such
/// value (≤ 100; a larger number is treated as a mis-parse and skipped). Lenient about whatever
/// precedes the digits (bar glyphs, spaces).
pub(crate) fn parse_pct(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c != b'%' {
            continue;
        }
        let mut j = i;
        while j > 0 && b[j - 1].is_ascii_digit() {
            j -= 1;
        }
        if j < i {
            if let Ok(n) = s[j..i].parse::<u32>() {
                if n <= 100 {
                    return Some(n as u8);
                }
            }
        }
    }
    None
}

/// Resolve a reset moment from text that may carry a date (`"jun 21 at 7:59pm"`) or just a clock
/// time (`"resets 11:59pm"`). Date-bearing strings resolve to that calendar day (this year, or
/// next year if already well past); bare times use [`parse_reset_at`]'s today/tomorrow logic.
/// Input should be lowercased.
pub(crate) fn parse_reset_datetime(lower: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    parse_dated(lower, now).or_else(|| parse_reset_at(lower, now))
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];

/// Parse a `"<month> <day> … <time>"` reset, e.g. `"resets jun 21 at 7:59pm"`. Returns `None`
/// when no month name is present (the bare-time path handles that).
fn parse_dated(lower: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    for (mi, m) in MONTHS.iter().enumerate() {
        let Some(pos) = lower.find(m) else { continue };
        let after = &lower[pos + m.len()..];
        let day = first_int(after)?;
        let time = find_reset_time(after)?;
        let month = mi as u32 + 1;
        let mut dt = build_local(now.year(), month, day, time)?;
        // A date well in the past means the printed month/day is next year's occurrence.
        if now - dt > Duration::hours(GRACE_PAST_HOURS) {
            dt = build_local(now.year() + 1, month, day, time)?;
        }
        return Some(dt.with_timezone(&Utc));
    }
    None
}

/// The first standalone integer in `s` (a day-of-month). Skips digits that are part of a clock
/// time (`7:59`) by requiring the integer not be immediately followed by `:`.
fn first_int(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_digit()) {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            // Not a clock time's hour, and a plausible day.
            if b.get(i) != Some(&b':') {
                if let Ok(n) = s[start..i].parse::<u32>() {
                    if (1..=31).contains(&n) {
                        return Some(n);
                    }
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

fn build_local(year: i32, month: u32, day: u32, time: NaiveTime) -> Option<DateTime<Local>> {
    let date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
    Local.from_local_datetime(&date.and_time(time)).earliest()
}

/// Claude states the *next* reset, at most a few hours out, in the machine's local timezone. So
/// today's occurrence is the answer when it's upcoming **or only recently passed** (within
/// [`GRACE_PAST_HOURS`]); only a time *well* in the past (a cross-midnight "resets 3am" seen at
/// night) rolls to tomorrow. Returns `None` if no clock time is present. Input should be lowercased.
pub(crate) fn parse_reset_at(lower: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
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
pub(crate) fn find_reset_time(lower: &str) -> Option<NaiveTime> {
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
pub(crate) fn parse_first_time(s: &str) -> Option<NaiveTime> {
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
pub(crate) fn try_parse_time_at(b: &[u8], start: usize) -> Option<NaiveTime> {
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
    use chrono::Timelike;

    #[test]
    fn parse_pct_basic() {
        assert_eq!(parse_pct("███▌   15% used"), Some(15));
        assert_eq!(parse_pct("   0% used"), Some(0));
        assert_eq!(parse_pct("100% used"), Some(100));
        assert_eq!(parse_pct("no percent here"), None);
        assert_eq!(parse_pct("999% bogus"), None); // >100 → mis-parse, skipped
    }

    #[test]
    fn dated_reset_resolves_to_that_day() {
        let now = Local.with_ymd_and_hms(2026, 6, 18, 20, 0, 0).unwrap();
        let dt = parse_reset_datetime("resets jun 21 at 7:59pm (asia/karachi)", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!((dt.month(), dt.day()), (6, 21));
        assert_eq!((dt.hour(), dt.minute()), (19, 59));
    }

    #[test]
    fn bare_time_falls_back_to_today() {
        let now = Local.with_ymd_and_hms(2026, 6, 18, 20, 0, 0).unwrap();
        let dt = parse_reset_datetime("resets 11:59pm (asia/karachi)", now)
            .unwrap()
            .with_timezone(&Local);
        assert_eq!(dt.day(), 18);
        assert_eq!((dt.hour(), dt.minute()), (23, 59));
    }
}
