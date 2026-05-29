//! `repomond` — the repomon background daemon.
//!
//! Owns the SQLite store, file watchers, git layer, and tmux-backed agent runtime, and
//! exposes them to clients over a length-prefixed JSON-RPC 2.0 Unix-socket API.

fn main() {
    // Scaffold entry point; the socket server and background services land in M4.
    println!("repomond {} (scaffold)", repomon_core::version());
}
