//! `repomon-core` — the engine behind repomon.
//!
//! This crate holds the data model, the gix-backed git layer, the SQLite store, the file
//! watchers, and the tmux-backed agent runtime. It contains no UI, no socket code, and no
//! daemon wiring — those live in `repomon-daemon` and `repomon-tui`, which both build on
//! the traits and types defined here.

pub mod config;
pub mod error;
pub mod git;
pub mod lane;
pub mod model;
pub mod registry;
pub mod store;
pub mod traits;
pub mod watch;

pub use config::Config;
pub use error::{Error, Result};
pub use lane::Lanes;
pub use model::*;
pub use registry::Registry;
pub use store::Store;
pub use traits::{LaneManager, RepoRegistry};
pub use watch::{ChangeKind, RepoChange, Watcher};

/// The crate (and product) version, surfaced via `daemon.status`.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
