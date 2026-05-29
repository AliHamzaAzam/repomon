//! `repomon-core` — the engine behind repomon.
//!
//! This crate holds the data model, the gix-backed git layer, the SQLite store, the file
//! watchers, and the tmux-backed agent runtime. It contains no UI, no socket code, and no
//! daemon wiring — those live in `repomon-daemon` and `repomon-tui`, which both build on
//! the traits and types defined here.

/// The crate (and product) version, surfaced via `daemon.status`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
