//! Runs the Windows-only agent host while retaining a compilable entry point on Unix.

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    repomon_host::windows_main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("repomon-agent-host runs only on Windows; on macOS/Linux repomon uses tmux.");
    std::process::ExitCode::from(2)
}
