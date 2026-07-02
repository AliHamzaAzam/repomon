//! The daemon client used by the TUI and headless CLI.
//!
//! The implementation now lives in [`repomon_core::client`] so the MCP server (`repomond mcp`)
//! shares the exact same wire framing, timeout, and event demuxing. This module re-exports it
//! to keep `crate::client::DaemonClient` working across the TUI.

pub use repomon_core::client::DaemonClient;
