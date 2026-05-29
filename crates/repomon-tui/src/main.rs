//! `repomon` — the terminal UI client.
//!
//! A thin client over the daemon socket: it renders cached fleet state and forwards input,
//! never calling git or tmux directly. The interactive four-zoom UI lands in M5/M9.

fn main() {
    // Scaffold entry point; the socket client and TUI land in M5.
    println!("repomon {} (scaffold)", repomon_core::version());
}
