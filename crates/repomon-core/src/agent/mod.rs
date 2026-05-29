//! Agent runtime and monitors.
//!
//! [`tmux`] provides the durable, tmux-backed runtime (spawn/capture/send/kill). Agent
//! session *monitoring* (deriving status and "needs you" from a Claude transcript) lands in
//! M8 alongside the `AgentMonitor` trait.

pub mod tmux;

pub use tmux::{shell_quote, TmuxRuntime};
