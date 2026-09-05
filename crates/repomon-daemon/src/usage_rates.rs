//! The daily LiteLLM price refresh: fetch, cache, and report status for `usage.rates` /
//! `usage.refresh_rates`.
//!
//! Deliberately its own module rather than living inside [`crate::usage_ingest`]: that module is
//! a concurrent-edit hotspot (the Claude transcript reader and source discovery). This module
//! owns the network fetch and its metadata; the fetched snapshot body itself lands at
//! [`crate::usage_ingest::price_cache_path`], the same file [`crate::usage_ingest::price_table`]
//! already reads when `[usage] refresh_prices` is on, so the two modules cooperate through one
//! shared file path rather than a direct dependency in either direction.
//!
//! ```text
//! spawn_daily_task ── every tick ──> is_due? / retry.due? ──> run_refresh ──> writes:
//!   - crate::usage_ingest::price_cache_path()   (the snapshot body; price_table() reads this)
//!   - usage_rates::meta_cache_path()            (etag / fetched_at / last_error)
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use repomon_core::config;
use repomon_core::pricing::RatesStatus;
use serde::{Deserialize, Serialize};

use crate::Ctx;

/// How often the daily refresh runs, and how stale the cache may get before it counts as due.
pub const REFRESH_INTERVAL: chrono::Duration = chrono::Duration::hours(24);
/// How long a single fetch may take before giving up.
const FETCH_TIMEOUT: StdDuration = StdDuration::from_secs(5);
/// How long after a refresh still leaves models unpriced before the one forced retry fires.
pub const UNPRICED_RETRY_DELAY: chrono::Duration = chrono::Duration::minutes(10);
/// How often the background task wakes to check whether a refresh (daily or retry) is due.
const POLL_INTERVAL: StdDuration = StdDuration::from_secs(60);

/// Where the fetch's metadata (etag, fetched/attempted timestamps, last error) is cached,
/// beside the price snapshot itself, which lives at `usage_ingest::price_cache_path()`.
pub fn meta_cache_path() -> PathBuf {
    config::data_dir().join("prices/litellm_meta.json")
}

/// Persisted fetch metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct CacheMeta {
    /// The cached snapshot's ETag, sent back as `If-None-Match` on the next fetch.
    etag: Option<String>,
    /// When a fetch last landed a 200 or a 304 (either counts: a 304 confirms the cache is
    /// current). `None` until the first successful attempt.
    fetched_at: Option<DateTime<Utc>>,
    /// When a fetch was last attempted at all, success or failure. This, not `fetched_at`, is
    /// what gates the daily cadence, so a failing endpoint is retried once a day rather than
    /// hammered every poll tick.
    last_attempt_at: Option<DateTime<Utc>>,
    /// The last attempt's error, if it failed. Cleared by the next successful attempt.
    last_error: Option<String>,
}

impl CacheMeta {
    fn load() -> Self {
        std::fs::read_to_string(meta_cache_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let path = meta_cache_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Whether a refresh is due, given when one was last attempted (success or failure).
pub fn is_due(last_attempt_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    match last_attempt_at {
        Some(t) => now - t >= REFRESH_INTERVAL,
        None => true,
    }
}

/// Tracks the single ten-minute retry a caller schedules when a refresh leaves models unpriced.
///
/// Not persisted across a daemon restart: a restart is itself a fresh chance, and if the model is
/// genuinely still unpriced the next `note_unpriced` call (from the next ledger read) schedules a
/// new one anyway. One retry per gap, not a loop: `mark_fired` spends it even if the retry itself
/// doesn't resolve the gap, so a permanently-unpriced model doesn't refetch every ten minutes
/// forever.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetryTracker {
    pending: Option<DateTime<Utc>>,
    fired: bool,
}

impl RetryTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Call after a ledger read finds at least one unpriced model. Schedules one retry ten
    /// minutes out, unless one is already pending or has already fired for this gap.
    pub fn note_unpriced(&mut self, now: DateTime<Utc>) {
        if self.pending.is_none() && !self.fired {
            self.pending = Some(now + UNPRICED_RETRY_DELAY);
        }
    }

    /// Call when a ledger read finds nothing unpriced, so a later gap gets its own fresh retry.
    pub fn clear(&mut self) {
        self.pending = None;
        self.fired = false;
    }

    /// Whether the scheduled retry should fire now.
    pub fn due(&self, now: DateTime<Utc>) -> bool {
        !self.fired && self.pending.is_some_and(|t| now >= t)
    }

    /// Spend the one retry. `due` reports `false` from here on until `clear` resets it.
    pub fn mark_fired(&mut self) {
        self.fired = true;
        self.pending = None;
    }

    #[cfg(test)]
    fn pending_at(&self) -> Option<DateTime<Utc>> {
        self.pending
    }
}

/// The daemon-held state behind `usage.rates` / `usage.refresh_rates`.
///
/// Only the in-memory, restart-safe-to-lose piece (the unpriced-model retry) lives here. The
/// fetch metadata (etag / timestamps / last error) is not cached in `Ctx`; it's read fresh from
/// disk on every `status()`/`run_refresh()` call instead, the same way `usage_ingest::price_table`
/// re-reads its snapshot file rather than caching it in `Ctx`. That keeps `Ctx::new` free of disk
/// I/O against the real data dir (tests construct a `Ctx` without an isolated `REPOMON_DATA_DIR`
/// far more often than they set one), at the cost of one small file read per call, cheap next to
/// the 24h cadence this is on.
#[derive(Default)]
pub struct RatesRuntime {
    pub retry: RetryTracker,
}

impl RatesRuntime {
    pub fn new() -> Self {
        Self::default()
    }
}

/// One fetch's outcome.
enum FetchOutcome {
    /// 304: the cached snapshot is still current.
    NotModified { etag: Option<String> },
    /// 200: a new snapshot body, and its ETag if the server sent one.
    Modified { body: String, etag: Option<String> },
}

/// One GET against `url`, sending `etag` as `If-None-Match` when present. `Ok(NotModified)` on a
/// 304, `Ok(Modified)` on a 200, `Err` for anything else (network failure, timeout, non-2xx).
fn fetch(url: &str, etag: Option<&str>) -> Result<FetchOutcome, String> {
    let agent = ureq::AgentBuilder::new().timeout(FETCH_TIMEOUT).build();
    let mut req = agent.get(url);
    if let Some(etag) = etag {
        req = req.set("If-None-Match", etag);
    }
    let resp = req.call().map_err(|e| e.to_string())?;
    let status = resp.status();
    let etag_out = resp.header("ETag").map(|s| s.to_string());
    if status == 304 {
        return Ok(FetchOutcome::NotModified { etag: etag_out });
    }
    let body = resp
        .into_string()
        .map_err(|e| format!("reading response body: {e}"))?;
    Ok(FetchOutcome::Modified {
        body,
        etag: etag_out,
    })
}

/// Write a fetched snapshot body to the same path `usage_ingest::price_table` reads.
fn write_snapshot(body: &str) -> std::io::Result<()> {
    let path = crate::usage_ingest::price_cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, body)
}

/// Run one fetch attempt now, unconditionally (the daily/retry cadence is the caller's job, see
/// [`spawn_daily_task`]). Updates the cached metadata and, on a 200 that parses, the cached
/// snapshot body.
pub async fn run_refresh(ctx: &Arc<Ctx>) {
    let url = {
        let config = ctx.config.read().await;
        config
            .usage
            .price_url
            .clone()
            .unwrap_or_else(|| repomon_core::config::DEFAULT_USAGE_PRICE_URL.to_string())
    };
    let mut meta = CacheMeta::load();
    let etag = meta.etag.clone();
    let now = Utc::now();
    let outcome = tokio::task::spawn_blocking(move || fetch(&url, etag.as_deref()))
        .await
        .unwrap_or_else(|e| Err(format!("refresh task panicked: {e}")));

    meta.last_attempt_at = Some(now);
    match outcome {
        Ok(FetchOutcome::NotModified { etag }) => {
            meta.fetched_at = Some(now);
            meta.last_error = None;
            if etag.is_some() {
                meta.etag = etag;
            }
        }
        Ok(FetchOutcome::Modified { body, etag }) => {
            match repomon_core::pricing::parse_litellm_snapshot(&body, now) {
                Ok(_) => match write_snapshot(&body) {
                    Ok(()) => {
                        meta.fetched_at = Some(now);
                        meta.last_error = None;
                        meta.etag = etag;
                    }
                    Err(e) => {
                        meta.last_error = Some(format!("caching the snapshot failed: {e}"));
                    }
                },
                Err(e) => {
                    meta.last_error = Some(format!("LiteLLM snapshot did not parse: {e}"));
                }
            }
        }
        Err(e) => {
            meta.last_error = Some(e);
        }
    }
    meta.save();
}

/// What the ledger knows about its price rates right now, for `usage.rates` and the CLI/desktop
/// surfaces that read it.
pub async fn status(ctx: &Arc<Ctx>) -> RatesStatus {
    let enabled = ctx.config.read().await.usage.refresh_prices;
    let table = crate::usage_ingest::price_table(ctx).await;
    let counts = table.source_counts();
    let meta = CacheMeta::load();
    RatesStatus {
        source_counts: counts,
        fetched_at: meta.fetched_at,
        etag: meta.etag,
        next_refresh_at: if enabled {
            meta.last_attempt_at.map(|t| t + REFRESH_INTERVAL)
        } else {
            None
        },
        last_error: meta.last_error,
        enabled,
    }
}

/// Record whether the ledger currently has unpriced models, for the ten-minute retry.
pub async fn note_unpriced(ctx: &Arc<Ctx>, unpriced: bool) {
    let mut rt = ctx.usage_rates.lock().await;
    if unpriced {
        rt.retry.note_unpriced(Utc::now());
    } else {
        rt.retry.clear();
    }
}

/// The background task: once a day (and once at startup, since the cache starts stale), fetch the
/// LiteLLM snapshot; between times, fire the one unpriced-model retry when it comes due. Reads
/// `[usage] refresh_prices` fresh every tick, so toggling it at runtime takes effect within one
/// poll interval without a daemon restart.
pub fn spawn_daily_task(ctx: Arc<Ctx>) {
    tokio::spawn(async move {
        loop {
            let enabled = ctx.config.read().await.usage.refresh_prices;
            if enabled {
                let now = Utc::now();
                let daily_due = is_due(CacheMeta::load().last_attempt_at, now);
                let retry_due = ctx.usage_rates.lock().await.retry.due(now);
                if daily_due || retry_due {
                    run_refresh(&ctx).await;
                    if retry_due {
                        ctx.usage_rates.lock().await.retry.mark_fired();
                    }
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    // --- is_due / cache age ---

    #[test]
    fn a_never_attempted_cache_is_due() {
        assert!(is_due(None, at(2026, 9, 1, 0)));
    }

    #[test]
    fn a_cache_attempted_moments_ago_is_not_due() {
        let now = at(2026, 9, 1, 12);
        assert!(!is_due(Some(now - chrono::Duration::minutes(5)), now));
    }

    #[test]
    fn a_cache_attempted_23h_ago_is_not_yet_due() {
        let now = at(2026, 9, 2, 0);
        assert!(!is_due(Some(now - chrono::Duration::hours(23)), now));
    }

    #[test]
    fn a_cache_attempted_24h_ago_is_due() {
        let now = at(2026, 9, 2, 0);
        assert!(is_due(Some(now - chrono::Duration::hours(24)), now));
    }

    #[test]
    fn a_cache_attempted_two_days_ago_is_due() {
        let now = at(2026, 9, 3, 0);
        assert!(is_due(Some(now - chrono::Duration::days(2)), now));
    }

    // --- RetryTracker: the retry for unpriced models ---

    #[test]
    fn no_retry_is_scheduled_until_something_is_unpriced() {
        let tracker = RetryTracker::new();
        assert!(!tracker.due(at(2026, 9, 1, 0)));
        assert_eq!(tracker.pending_at(), None);
    }

    #[test]
    fn an_unpriced_model_schedules_a_retry_ten_minutes_out() {
        let mut tracker = RetryTracker::new();
        let now = at(2026, 9, 1, 0);
        tracker.note_unpriced(now);
        assert_eq!(tracker.pending_at(), Some(now + UNPRICED_RETRY_DELAY));
        assert!(!tracker.due(now), "not due yet");
        assert!(!tracker.due(now + chrono::Duration::minutes(9)));
        assert!(tracker.due(now + UNPRICED_RETRY_DELAY));
    }

    #[test]
    fn a_second_unpriced_report_does_not_push_the_retry_later() {
        let mut tracker = RetryTracker::new();
        let now = at(2026, 9, 1, 0);
        tracker.note_unpriced(now);
        let first_deadline = tracker.pending_at();
        tracker.note_unpriced(now + chrono::Duration::minutes(5));
        assert_eq!(tracker.pending_at(), first_deadline);
    }

    #[test]
    fn the_retry_fires_once_and_does_not_reschedule_itself() {
        let mut tracker = RetryTracker::new();
        let now = at(2026, 9, 1, 0);
        tracker.note_unpriced(now);
        let due_at = now + UNPRICED_RETRY_DELAY;
        assert!(tracker.due(due_at));
        tracker.mark_fired();
        assert!(
            !tracker.due(due_at + chrono::Duration::hours(1)),
            "spent: still unpriced stays flagged rather than retried forever"
        );
        // Still unpriced after the retry: note_unpriced must not schedule another one.
        tracker.note_unpriced(due_at);
        assert!(!tracker.due(due_at + UNPRICED_RETRY_DELAY));
    }

    #[test]
    fn clearing_after_the_gap_resolves_lets_a_later_gap_retry_again() {
        let mut tracker = RetryTracker::new();
        let now = at(2026, 9, 1, 0);
        tracker.note_unpriced(now);
        tracker.mark_fired();
        tracker.clear();
        tracker.note_unpriced(now + chrono::Duration::hours(1));
        assert!(tracker.due(now + chrono::Duration::hours(1) + UNPRICED_RETRY_DELAY));
    }

    // --- fetch(): a local TCP server stands in for LiteLLM, no real network in tests ---

    /// Serve one HTTP/1.1 response per accepted connection, in order, and report each request's
    /// `If-None-Match` header (empty string when absent) back over the channel.
    fn spawn_http(
        responses: Vec<(u16, Vec<(&'static str, String)>, Vec<u8>)>,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (status, headers, body) in responses {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let inm = req
                    .lines()
                    .find_map(|l| {
                        let lower = l.to_ascii_lowercase();
                        lower.starts_with("if-none-match:").then(|| {
                            l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string()
                        })
                    })
                    .unwrap_or_default();
                let _ = tx.send(inm);
                let reason = match status {
                    200 => "OK",
                    304 => "Not Modified",
                    _ => "Error",
                };
                let mut resp = format!("HTTP/1.1 {status} {reason}\r\n");
                for (k, v) in &headers {
                    resp.push_str(&format!("{k}: {v}\r\n"));
                }
                resp.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (format!("http://{addr}"), rx)
    }

    #[test]
    fn fetch_returns_the_body_and_etag_on_200() {
        let (url, rx) = spawn_http(vec![(
            200,
            vec![("ETag", "\"abc\"".to_string())],
            br#"{"m":{"input_cost_per_token":1e-6,"output_cost_per_token":2e-6}}"#.to_vec(),
        )]);
        match fetch(&url, None).unwrap() {
            FetchOutcome::Modified { body, etag } => {
                assert!(body.contains("input_cost_per_token"));
                assert_eq!(etag.as_deref(), Some("\"abc\""));
            }
            FetchOutcome::NotModified { .. } => panic!("expected a 200"),
        }
        assert_eq!(rx.recv().unwrap(), "", "no If-None-Match without a cached etag");
    }

    #[test]
    fn fetch_sends_if_none_match_and_treats_304_as_fresh() {
        let (url, rx) = spawn_http(vec![(304, vec![], vec![])]);
        match fetch(&url, Some("\"abc\"")).unwrap() {
            FetchOutcome::NotModified { .. } => {}
            FetchOutcome::Modified { .. } => panic!("expected a 304"),
        }
        assert_eq!(rx.recv().unwrap(), "\"abc\"");
    }

    #[test]
    fn fetch_reports_an_error_for_a_server_failure() {
        let (url, _rx) = spawn_http(vec![(500, vec![], b"boom".to_vec())]);
        assert!(fetch(&url, None).is_err());
    }

    // --- status() shape, against a throwaway Ctx ---

    fn test_ctx() -> Arc<Ctx> {
        Ctx::new(
            repomon_core::Store::open_in_memory().unwrap(),
            repomon_core::Config::default(),
            None,
        )
    }

    #[tokio::test]
    async fn status_reports_disabled_with_no_fetch_timestamps() {
        let ctx = test_ctx();
        ctx.config.write().await.usage.refresh_prices = false;
        let status = status(&ctx).await;
        assert!(!status.enabled);
        assert_eq!(status.fetched_at, None);
        assert_eq!(status.next_refresh_at, None);
        assert!(status.source_counts.builtin > 0, "the built-in table is the floor");
    }

    #[tokio::test]
    async fn status_reports_a_forced_refresh_against_a_local_server() {
        let ctx = test_ctx();
        ctx.config.write().await.usage.refresh_prices = true;
        // REPOMON_DATA_DIR isn't touched here: run_refresh writes under the process's real data
        // dir via `config::data_dir()`. Isolate it so this test never touches the operator's
        // actual price cache.
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: single-threaded within this test's lifetime as far as this env var goes; other
        // tests in this file don't read REPOMON_DATA_DIR.
        unsafe { std::env::set_var("REPOMON_DATA_DIR", dir.path()) };

        let (url, _rx) = spawn_http(vec![(
            200,
            vec![("ETag", "\"snap-1\"".to_string())],
            br#"{"sample-model":{"input_cost_per_token":0.000003,"output_cost_per_token":0.000015}}"#
                .to_vec(),
        )]);
        ctx.config.write().await.usage.price_url = Some(url);

        run_refresh(&ctx).await;
        let status = status(&ctx).await;
        assert_eq!(status.etag.as_deref(), Some("\"snap-1\""));
        assert!(status.fetched_at.is_some());
        assert!(status.last_error.is_none(), "{:?}", status.last_error);
        assert!(status.next_refresh_at.is_some());
        assert!(
            status.source_counts.litellm >= 1,
            "the fetched snapshot's model should be counted, got {:?}",
            status.source_counts
        );

        unsafe { std::env::remove_var("REPOMON_DATA_DIR") };
    }

    #[tokio::test]
    async fn a_failed_refresh_is_surfaced_as_last_error() {
        let ctx = test_ctx();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("REPOMON_DATA_DIR", dir.path()) };
        ctx.config.write().await.usage.refresh_prices = true;
        // Nothing listens on this port.
        ctx.config.write().await.usage.price_url = Some("http://127.0.0.1:1".to_string());

        run_refresh(&ctx).await;
        let status = status(&ctx).await;
        assert!(status.last_error.is_some());
        assert!(status.fetched_at.is_none());

        unsafe { std::env::remove_var("REPOMON_DATA_DIR") };
    }
}
