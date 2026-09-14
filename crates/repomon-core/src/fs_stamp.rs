//! When a filesystem timestamp may be trusted as a change detector.
//!
//! Caches all over this workspace memoise work keyed on a file's or directory's modification time,
//! on the reasoning that an unchanged stamp means unchanged contents. That reasoning only holds
//! once the stamp is old enough that a later change is forced to carry a different one.

use std::time::{Duration, SystemTime};

/// How far in the past a stamp must already be before "unchanged stamp" is evidence of "unchanged
/// contents".
///
/// Three separate mechanisms put a floor under this, and the coarsest wins:
///
/// - FAT and exFAT record write times in 2-second units, measured on a FAT32 volume on macOS: a
///   create and the remove that undid it both left the directory's stamp untouched.
/// - Windows stamps files from the system clock, whose interrupt period is typically 10 ms to
///   20 ms, so changes inside one tick share a stamp even on NTFS.
/// - Microsoft promises only that "the file time is correctly reflected when the handle that makes
///   the change is closed", so a writer still holding the file open may not have moved it at all.
pub const STAMP_SETTLE: Duration = Duration::from_secs(2);

/// Whether `stamp` is old enough, as of `observed_at`, that any change landing at or after
/// `observed_at` must carry a stamp distinguishable from it.
///
/// `observed_at` must be read *before* the work whose result is about to be memoised, not after.
/// Taken afterwards it would excuse a change that landed during that work and shared `stamp`'s
/// tick, which is the exact window this guards.
///
/// A stamp from the future, which is what clock skew against a network mount looks like, is never
/// settled: such a mount redoes the work rather than trusting a stamp that may never move.
pub fn is_settled(stamp: SystemTime, observed_at: SystemTime) -> bool {
    observed_at
        .duration_since(stamp)
        .is_ok_and(|age| age >= STAMP_SETTLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic ages, so the boundary holds identically on every filesystem and every platform.
    #[test]
    fn a_stamp_is_settled_only_once_it_outlives_the_coarsest_write_time_resolution() {
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for (age, settled) in [(0, false), (1, false), (2, true), (3600, true)] {
            assert_eq!(
                is_settled(stamp, stamp + Duration::from_secs(age)),
                settled,
                "a stamp {age}s in the past"
            );
        }
    }

    /// Clock skew against a network mount reads as a stamp from the future, never as settled.
    #[test]
    fn a_stamp_from_the_future_is_never_settled() {
        let stamp = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert!(!is_settled(stamp, stamp - Duration::from_secs(1)));
        assert!(!is_settled(stamp, stamp - Duration::from_secs(86_400)));
    }
}
