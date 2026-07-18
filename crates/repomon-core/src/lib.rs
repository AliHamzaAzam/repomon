//! `repomon-core` — the engine behind repomon.
//!
//! This crate holds the data model, the gix-backed git layer, the SQLite store, the file
//! watchers, the tmux-backed agent runtime, and the shared [`client::DaemonClient`] every
//! out-of-process consumer uses to reach the daemon's socket. It contains no UI and no daemon
//! wiring — those live in `repomon-daemon` and `repomon-tui`, which both build on the traits
//! and types defined here.

pub mod agent;
pub mod analytics;
pub mod client;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod exec;
pub mod git;
pub mod indexer;
pub mod lane;
pub mod model;
pub mod notify;
pub mod protocol;
pub mod registry;
pub mod service;
pub mod session;
pub mod store;
pub mod traits;
pub mod transport;
pub mod watch;

pub use agent::{AgentMonitor, ClaudeMonitor, TmuxRuntime};
pub use config::Config;
pub use error::{Error, Result};
pub use indexer::Indexer;
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
