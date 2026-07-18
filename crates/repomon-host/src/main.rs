//! `repomon-agent-host` binary entry point. See `PROTOCOL.md` for the contract this
//! process implements. The binary compiles everywhere so the workspace builds on all
//! OSes, but the runtime is Windows-only (Unix uses tmux).

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    repomon_host::windows_main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("repomon-agent-host runs only on Windows; on macOS/Linux repomon uses tmux.");
    std::process::ExitCode::from(2)
}
