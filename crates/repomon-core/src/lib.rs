//! Shares the data model, repository and storage operations, platform session runtimes, and daemon
//! client across applications.

pub mod agent;
pub mod analytics;
pub mod client;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod exec;
pub mod git;
pub mod indexer;
pub mod input;
pub mod lane;
pub mod launch;
pub mod local_llm;
pub mod model;
pub mod notes;
pub mod notify;
pub mod pricing;
pub mod process;
pub mod protocol;
pub mod registry;
pub mod schedule;
pub mod service;
pub mod session;
pub mod store;
pub mod traits;
pub mod transport;
pub mod usage_ledger;
pub mod watch;

#[cfg(windows)]
pub use agent::WindowsBackend;
pub use agent::{
    AgentMonitor, ByteStreamEvent, ClaudeMonitor, SessionBackend, TmuxRuntime, tmux_program,
};
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
