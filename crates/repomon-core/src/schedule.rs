//! Standing-orchestration schedule specs: a tiny, human-first grammar instead of cron.
//!
//! Supported: `daily HH:MM`, `weekdays HH:MM`, `weekends HH:MM`, `every <N>m`, `every <N>h`.
//! Times are the daemon host's local time. Parsing is strict and errors carry examples, since
//! the spec usually arrives from a human typing `repomon orchestrate --schedule "..."`.

use chrono::{DateTime, Datelike, Duration, Local, NaiveTime, TimeZone, Weekday};

use crate::error::{Error, Result};

/// A parsed schedule spec. Stored as its original string; re-parsed (infallibly, post-add
/// validation) wherever the next firing time is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Spec {
    Daily { h: u32, m: u32 },
    Weekdays { h: u32, m: u32 },
    Weekends { h: u32, m: u32 },
    Every { minutes: i64 },
}

/// Parse a spec string. Errors name the accepted forms so a typo teaches the grammar.
pub fn parse_spec(s: &str) -> Result<Spec> {
    let err = || {
        Error::Config(format!(
            "bad schedule spec {s:?} — use \"daily HH:MM\", \"weekdays HH:MM\", \
             \"weekends HH:MM\", \"every 30m\", or \"every 2h\""
        ))
    };
    let mut parts = s.split_whitespace();
    let (kind, rest) = (parts.next().ok_or_else(err)?, parts.next().ok_or_else(err)?);
    if parts.next().is_some() {
        return Err(err());
    }
    match kind.to_ascii_lowercase().as_str() {
        "daily" | "weekdays" | "weekends" => {
            let (h, m) = rest.split_once(':').ok_or_else(err)?;
            let h: u32 = h.parse().map_err(|_| err())?;
            let m: u32 = m.parse().map_err(|_| err())?;
            if h > 23 || m > 59 {
                return Err(err());
            }
            Ok(match kind.to_ascii_lowercase().as_str() {
                "daily" => Spec::Daily { h, m },
                "weekdays" => Spec::Weekdays { h, m },
                _ => Spec::Weekends { h, m },
            })
        }
        "every" => {
            let (num, unit) = rest.split_at(rest.len().saturating_sub(1));
            let n: i64 = num.parse().map_err(|_| err())?;
            if n <= 0 {
                return Err(err());
            }
            match unit {
                "m" => Ok(Spec::Every { minutes: n }),
                "h" => Ok(Spec::Every { minutes: n * 60 }),
                _ => Err(err()),
            }
        }
        _ => Err(err()),
    }
}

impl Spec {
    /// The first firing strictly after `after`.
    pub fn next_after(&self, after: DateTime<Local>) -> DateTime<Local> {
        match self {
            Spec::Every { minutes } => after + Duration::minutes(*minutes),
            Spec::Daily { h, m } => next_at(after, *h, *m, |_| true),
            Spec::Weekdays { h, m } => next_at(after, *h, *m, |d| {
                !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
            }),
            Spec::Weekends { h, m } => next_at(after, *h, *m, |d| {
                matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
            }),
        }
    }
}

/// The next `h:m` local time strictly after `after` on a day passing `day_ok`.
fn next_at(
    after: DateTime<Local>,
    h: u32,
    m: u32,
    day_ok: impl Fn(&DateTime<Local>) -> bool,
) -> DateTime<Local> {
    let target = NaiveTime::from_hms_opt(h, m, 0).expect("validated at parse");
    let mut day = after.date_naive();
    // Same-day candidate only when the time is still ahead.
    if after.time() >= target {
        day = day.succ_opt().expect("date overflow");
    }
    for _ in 0..14 {
        // DST gaps: from_local_datetime can be ambiguous or skipped; earliest() falls back to
        // the next valid instant via latest() — either way we land on a real local time.
        let naive = day.and_time(target);
        if let Some(dt) = Local
            .from_local_datetime(&naive)
            .earliest()
            .or_else(|| Local.from_local_datetime(&naive).latest())
        {
            if day_ok(&dt) && dt > after {
                return dt;
            }
        }
        day = day.succ_opt().expect("date overflow");
    }
    // Unreachable for the supported specs (every 14-day window has weekdays and weekends);
    // fall back to a day out rather than panic.
    after + Duration::days(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(y, mo, d)
                    .unwrap()
                    .and_hms_opt(h, mi, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
    }

    #[test]
    fn parses_the_documented_forms() {
        assert_eq!(
            parse_spec("daily 09:00").unwrap(),
            Spec::Daily { h: 9, m: 0 }
        );
        assert_eq!(
            parse_spec("weekdays 9:00").unwrap(),
            Spec::Weekdays { h: 9, m: 0 }
        );
        assert_eq!(
            parse_spec("weekends 21:30").unwrap(),
            Spec::Weekends { h: 21, m: 30 }
        );
        assert_eq!(
            parse_spec("every 30m").unwrap(),
            Spec::Every { minutes: 30 }
        );
        assert_eq!(
            parse_spec("every 2h").unwrap(),
            Spec::Every { minutes: 120 }
        );
        assert_eq!(
            parse_spec("DAILY 09:00").unwrap(),
            Spec::Daily { h: 9, m: 0 }
        );
    }

    #[test]
    fn rejects_bad_specs_with_examples() {
        for bad in [
            "tuesdays 09:00",
            "daily 25:00",
            "daily 09:61",
            "every 0m",
            "every -5m",
            "every 5d",
            "daily",
            "",
            "daily 09:00 extra",
        ] {
            let err = parse_spec(bad).unwrap_err().to_string();
            assert!(
                err.contains("daily HH:MM"),
                "unhelpful error for {bad:?}: {err}"
            );
        }
    }

    #[test]
    fn every_fires_a_fixed_interval_later() {
        let t = local(2026, 7, 20, 10, 0);
        assert_eq!(
            Spec::Every { minutes: 30 }.next_after(t),
            t + Duration::minutes(30)
        );
    }

    #[test]
    fn daily_rolls_to_tomorrow_when_past() {
        // 2026-07-20 is a Monday.
        let spec = Spec::Daily { h: 9, m: 0 };
        assert_eq!(
            spec.next_after(local(2026, 7, 20, 8, 0)),
            local(2026, 7, 20, 9, 0)
        );
        assert_eq!(
            spec.next_after(local(2026, 7, 20, 10, 0)),
            local(2026, 7, 21, 9, 0)
        );
        // Exactly at the target time counts as past (strictly-after contract).
        assert_eq!(
            spec.next_after(local(2026, 7, 20, 9, 0)),
            local(2026, 7, 21, 9, 0)
        );
    }

    #[test]
    fn weekdays_skips_the_weekend() {
        // 2026-07-24 is a Friday.
        let spec = Spec::Weekdays { h: 9, m: 0 };
        assert_eq!(
            spec.next_after(local(2026, 7, 24, 10, 0)),
            local(2026, 7, 27, 9, 0),
            "Friday past 9am must fire Monday"
        );
    }

    #[test]
    fn weekends_skips_to_saturday() {
        let spec = Spec::Weekends { h: 9, m: 0 };
        assert_eq!(
            spec.next_after(local(2026, 7, 20, 10, 0)),
            local(2026, 7, 25, 9, 0),
            "Monday must fire next Saturday"
        );
    }
}
