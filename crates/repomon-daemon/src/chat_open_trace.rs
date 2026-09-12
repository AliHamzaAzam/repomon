//! Shipped instrumentation for two operator-reported faults that only happen on his machine: a
//! slow first chat open after a daemon start, and every pane stalling at once while the rest of
//! the app stays responsive.
//!
//! Scope: this is a probe, not an RPC trace. Chat opens produce a handful of lines and only for
//! the first opens after a daemon start. Stalls produce a line only past
//! [`SLOW_MS`], so a healthy daemon writes none at all. Everything lands in one bounded file
//! under the data directory. It never writes to stdout, because the desktop spawns the daemon and
//! that output goes nowhere readable.
//!
//! Removing it is one commit: delete this file, its `mod` line in `lib.rs`, the `install()` call
//! in `main.rs`, the `depth`/`running` fields, `queue_depth` and the `SlowCallHook` in
//! `repomon-core`'s store, the `trace` parameter on `transcript::resolve_source`, the oneshot
//! payload in `socket.rs` (back to `channel()` and `drop(done)`), and every line matching
//! `chat_open_trace::`. Each call site is a single statement so the deletion is mechanical.

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
    write_line(fields);
}

/// The append itself, shared by the chat-open and stall budgets.
fn write_line(fields: std::fmt::Arguments<'_>) {
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

/// A stall line is only written past this wait, so normal use produces none at all.
const SLOW_MS: f64 = 250.0;
/// Stall lines get their own budget so a burst of them cannot hide the chat-open lines.
const MAX_STALLS_PER_START: usize = 256;
static STALLS: AtomicUsize = AtomicUsize::new(0);

fn stall(fields: std::fmt::Arguments<'_>) {
    if STALLS.fetch_add(1, Ordering::Relaxed) >= MAX_STALLS_PER_START {
        return;
    }
    write_line(fields);
}

/// An ordered request that waited behind its predecessor on the same connection. The desktop
/// drives every pane over one connection, so this is what a whole-fleet pane stall looks like
/// from the socket's side: `after` names the method that held the chain and `after_ms` is how
/// long it held it.
pub fn ordered_wait(method: &str, waited_ms: f64, after: &str, after_ms: f64) {
    if waited_ms <= SLOW_MS {
        return;
    }
    stall(format_args!(
        "ordered_wait method={method} waited_ms={waited_ms:.1} after={after} after_ms={after_ms:.1}"
    ));
}

/// A store call that waited for the single worker thread. `running` names the job that was in
/// front of it and `depth` how many were queued, which separates one slow job from a deep queue.
pub fn store_wait(job: &'static str, waited_ms: f64, depth: usize, running: &'static str) {
    stall(format_args!(
        "store_wait job={} waited_ms={waited_ms:.1} depth={depth} running={}",
        short(job),
        short(running)
    ));
}

/// `type_name` gives a fully qualified closure path; the method name is the useful part.
fn short(name: &'static str) -> &'static str {
    name.strip_suffix("::{{closure}}")
        .unwrap_or(name)
        .rsplit_once("::")
        .map_or(name, |(_, tail)| tail)
}

/// Install the store reporter. Called once at daemon start.
pub fn install() {
    repomon_core::store::set_slow_call_hook(store_wait);
}

/// A submitted input still pinned long after it was sent. Consumption needs `same_source` and
/// either a matching mail id or (`after_send` and equal text); this names which of those refused
/// the best candidate row, so a stuck brief identifies its own cause. One line per ticket.
pub fn input_stuck(
    age_ms: i64,
    user_rows: usize,
    same_source: bool,
    after_send: bool,
    text_equal: bool,
    chars: usize,
) {
    stall(format_args!(
        "input_stuck age_ms={age_ms} user_rows={user_rows} same_source={same_source} \
         after_send={after_send} text_equal={text_equal} chars={chars}"
    ));
}

/// An ordered request still running, reported while it runs rather than after it finishes.
///
/// `ordered_wait` can only ever describe a stall that already ended: it is written by the
/// successor once its predecessor completes and reports its own duration. A request that never
/// returns therefore leaves no line at all, its successors' clients give up at their own ceiling,
/// and the trace shows only the worst stall that happened to finish. This is the other half.
pub fn chain_head(method: &str, elapsed_ms: u64, connection: u64) {
    stall(format_args!(
        "chain_head method={method} elapsed_ms={elapsed_ms} conn={connection} still_running=true"
    ));
}

/// A catch-up read found evidence the display page could not reach. Pairs with `input_stuck`:
/// that line names a ticket nothing could retire, this one names the read that retired it.
pub fn input_caught_up(retired: usize, floor: u64, rows: usize) {
    stall(format_args!(
        "input_caught_up retired={retired} floor={floor} rows={rows}"
    ));
}

/// Built once per daemon start, on the first conversation that has costed events to price.
pub fn price_table(since: Instant, models: usize) {
    write(format_args!(
        "price_table build_ms={:.1} models={models}",
        millis(since)
    ));
}
