//! Windows attach pop-out: launch the raw byte-proxy attach client (`repomon attach-host
//! <window>`) in a *separate* terminal so the FleetView TUI keeps running.
//!
//! On macOS/Linux "attach" hands the current terminal over to tmux (see [`app::run_attach`] —
//! it blocks until the user detaches). The TUI has no terminal to give up on Windows, so it
//! pops the agent out instead: into a titled Windows Terminal tab when `wt.exe` is on PATH, and
//! otherwise a brand-new console (`CREATE_NEW_CONSOLE`). Both keep the embedded focus-view
//! renderer live alongside them (decision #2 in the plan: embedded + external window).
//!
//! The launcher choice and the `wt.exe` argv are pure logic, unit-tested on every OS; only the
//! process spawn is `#[cfg(windows)]`.
//!
//! [`app::run_attach`]: crate::app

/// `CREATE_NEW_CONSOLE`: the fallback attach client gets its own console window rather than
/// sharing the TUI's (which its raw-VT takeover would corrupt). The `wt.exe` path needs no such
/// flag — Windows Terminal hosts the client in its own tab's pseudoconsole.
#[cfg(windows)]
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

/// Where to pop the attach client out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launcher {
    /// `wt.exe` is on PATH: open the client in a titled new Windows Terminal tab.
    WindowsTerminal,
    /// No Windows Terminal: spawn the client in a brand-new console window.
    NewConsole,
}

/// Prefer a Windows Terminal tab when `wt.exe` is available, else a fresh console.
pub fn choose_launcher(wt_on_path: bool) -> Launcher {
    if wt_on_path {
        Launcher::WindowsTerminal
    } else {
        Launcher::NewConsole
    }
}

/// The `wt.exe` argument vector that opens `program args…` in a titled new tab:
/// `new-tab --title <title> <program> <args…>`. Windows Terminal treats everything after the
/// `new-tab` options as the commandline to run, so no `--` terminator is used (nor needed — the
/// attach client's program and args never start with a dash).
pub fn wt_argv(title: &str, program: &str, args: &[String]) -> Vec<String> {
    let mut v = vec![
        "new-tab".to_string(),
        "--title".to_string(),
        title.to_string(),
        program.to_string(),
    ];
    v.extend(args.iter().cloned());
    v
}

/// Pop the attach client (`program args…`, i.e. `repomon attach-host <window>`) out into a
/// separate terminal, titled `title`. Returns once the launcher has been spawned — the TUI
/// never blocks on the popped-out window (unlike the Unix in-terminal attach).
#[cfg(windows)]
pub fn launch(title: &str, program: &str, args: &[String]) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    match choose_launcher(repomon_core::exec::find_in_path("wt").is_some()) {
        Launcher::WindowsTerminal => {
            // `wt.exe` is an app-execution alias; CreateProcess resolves it off PATH. It hands
            // the tab to the running Windows Terminal and exits, so this spawn is fire-and-forget
            // (never wait on it — the attach client lives under WindowsTerminal.exe, not us).
            Command::new("wt.exe")
                .args(wt_argv(title, program, args))
                .spawn()
                .map(drop)
                .map_err(|e| anyhow::anyhow!("couldn't open a Windows Terminal tab: {e}"))
        }
        Launcher::NewConsole => {
            // No stdio redirection: CREATE_NEW_CONSOLE allocates a fresh console for the child,
            // and the attach client needs those real console handles for its raw-VT takeover.
            // The child outlives the TUI (Windows never auto-reaps it), matching a pop-out tab.
            Command::new(program)
                .args(args)
                .creation_flags(CREATE_NEW_CONSOLE)
                .spawn()
                .map(drop)
                .map_err(|e| anyhow::anyhow!("couldn't open a new console: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chooses_windows_terminal_when_wt_is_on_path() {
        assert_eq!(choose_launcher(true), Launcher::WindowsTerminal);
        assert_eq!(choose_launcher(false), Launcher::NewConsole);
    }

    #[test]
    fn wt_argv_opens_a_titled_new_tab_running_the_client() {
        // The exact pop-out invocation: `wt.exe new-tab --title lane-3-1 repomon attach-host
        // lane-3-1`. No `--` terminator (WT reads the trailing tokens as the commandline).
        let args = wt_argv(
            "lane-3-1",
            "repomon",
            &["attach-host".to_string(), "lane-3-1".to_string()],
        );
        assert_eq!(
            args,
            vec![
                "new-tab",
                "--title",
                "lane-3-1",
                "repomon",
                "attach-host",
                "lane-3-1",
            ]
        );
    }

    #[test]
    fn wt_argv_preserves_a_title_with_spaces_as_one_token() {
        // Rust quotes args with spaces for CreateProcess; WT's own arg parse then reconstructs
        // the single `--title` token, so a lane label with spaces stays intact.
        let args = wt_argv("my feature", "repomon", &["attach-host".to_string()]);
        assert_eq!(args[2], "my feature");
    }
}
