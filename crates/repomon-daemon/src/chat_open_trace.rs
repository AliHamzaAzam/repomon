//! First-chat-open instrumentation, shipped so the next slow open is captured on the machine
//! where it actually happens.
//!
//! Scope: this is a first-open probe, not an RPC trace. It writes a handful of lines per chat
//! open, only for the first opens after a daemon start, to a bounded file under the data
//! directory. It never writes to stdout, because the desktop spawns the daemon and that output
//! goes nowhere readable.
//!
//! Removing it is one commit: delete this file, its `mod` line in `lib.rs`, `Store::queue_depth`
//! and its `depth` counter in `repomon-core`, the `trace` parameter on
//! `transcript::resolve_source`, and every line matching `chat_open_trace::`. Each call site is a
//! single statement so the deletion is mechanical.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// A daemon start is expected to produce a few lines. This is the ceiling for one process, so a
/// long-lived daemon cannot turn the file into a log.
const MAX_EVENTS_PER_START: usize = 64;
/// Hard ceiling on the file across daemon starts. Exceeding it restarts the file rather than
/// letting it grow.
const MAX_BYTES: u64 = 256 * 1024;

static EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Where the operator reads the result.
pub fn path() -> std::path::PathBuf {
    repomon_core::config::data_dir().join("chat-open-trace.log")
}

fn millis(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// The stages `resolve_source` spends its time in, accumulated in microseconds. Two relaxed
/// atomic adds on the path, and nothing at all when no trace is attached.
#[derive(Default)]
pub struct Stages {
    store_us: AtomicU64,
    windows_us: AtomicU64,
}

impl Stages {
    /// Time spent waiting on the single store worker thread.
    pub fn store(&self, since: Instant) {
        self.store_us
            .fetch_add(since.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    /// Time spent probing tmux for the window's age and metadata.
    pub fn windows(&self, since: Instant) {
        self.windows_us
            .fetch_add(since.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    fn store_ms(&self) -> f64 {
        self.store_us.load(Ordering::Relaxed) as f64 / 1000.0
    }

    fn windows_ms(&self) -> f64 {
        self.windows_us.load(Ordering::Relaxed) as f64 / 1000.0
    }
}

/// Append one line. Bounded by event count per start and by file size; silent on any IO error,
/// because instrumentation must never be able to fail a chat open.
fn write(fields: std::fmt::Arguments<'_>) {
    if EVENTS.fetch_add(1, Ordering::Relaxed) >= MAX_EVENTS_PER_START {
        return;
    }
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = std::fs::remove_file(&path);
    }
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let _ = writeln!(file, "{at} {fields}");
    }
}

/// The whole daemon-side cost of one chat open, broken into the stages that were measured
/// separately during the investigation. `uptime_ms` distinguishes the first open after a daemon
/// start from a later one; `store_depth` is the queue in front of this request's store calls at
/// the moment the watch arrived.
#[allow(clippy::too_many_arguments)]
pub fn chat_open(
    window: &str,
    kind: &str,
    uptime: std::time::Duration,
    store_depth: usize,
    total: Instant,
    source_ms: f64,
    stages: &Stages,
    page_ms: f64,
    pane_ms: f64,
    resolved: bool,
    items: usize,
) {
    write(format_args!(
        "chat_open window={window} kind={kind} uptime_ms={} store_depth={store_depth} \
         total_ms={:.1} source_ms={source_ms:.1} store_ms={:.1} windows_ms={:.1} \
         page_ms={page_ms:.1} pane_ms={pane_ms:.1} source={} items={items}",
        uptime.as_millis(),
        millis(total),
        stages.store_ms(),
        stages.windows_ms(),
        if resolved { "durable" } else { "pane" },
    ));
}

/// The background pass that establishes a window's provider binding when the ledger has not
/// already supplied one. It runs after the initial page, so a slow one delays the real
/// conversation appearing without delaying the response.
pub fn discovery(window: &str, kind: &str, since: Instant, resolved: bool) {
    write(format_args!(
        "chat_discovery window={window} kind={kind} ms={:.1} source={}",
        millis(since),
        if resolved { "durable" } else { "pane" },
    ));
}

/// Built once per daemon start, on the first conversation that has costed events to price.
pub fn price_table(since: Instant, models: usize) {
    write(format_args!(
        "price_table build_ms={:.1} models={models}",
        millis(since)
    ));
}
