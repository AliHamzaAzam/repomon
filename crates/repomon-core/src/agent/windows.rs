//! Windows session backend: per-window `repomon-agent-host.exe` processes.
//!
//! The Windows counterpart of [`TmuxRuntime`](super::tmux::TmuxRuntime): each agent window is
//! owned by one detached host process (ConPTY + child + server-side vt100 screen) that serves
//! the frozen control protocol in `crates/repomon-host/PROTOCOL.md` on
//! `\\.\pipe\repomon-<session>-<window>` and registers itself under
//! `<data_dir>\hosts\<session>\<window>.json`. Hosts survive daemon restarts; on startup the
//! backend re-adopts them by scanning the registry and `hello`-verifying each pipe — the
//! Windows equivalent of the daemon finding an existing tmux server.
//!
//! Everything decision-shaped (spawn-command parsing, host argv assembly, target formats,
//! scan adopt/skip/GC rules, shell selection) is pure logic tested on every OS; only the pipe
//! client, host spawning, and the byte-stream pump are `#[cfg(windows)]`.

use crate::error::{Error, Result};

use super::backend::AttachCommand;

// ---------------------------------------------------------------------------
// Pure logic (all OSes)
// ---------------------------------------------------------------------------

/// Split a [`SpawnSpec`](super::backend::SpawnSpec) `program` string into environment
/// assignments and an argv. On Unix the program is a shell fragment run via `sh -c`; there is
/// no shell on Windows, so the backend parses the common shapes itself: leading `KEY=VALUE`
/// tokens become environment overrides (`CLAUDE_CONFIG_DIR='…' claude`), and the rest is
/// whitespace-split with single/double quotes respected (quotes group, backslashes are plain
/// path characters). An empty program is an error.
pub fn split_spawn_program(program: &str) -> Result<(Vec<(String, String)>, Vec<String>)> {
    let tokens = tokenize(program);
    let mut env: Vec<(String, String)> = Vec::new();
    let mut argv: Vec<String> = Vec::new();
    for tok in tokens {
        if argv.is_empty()
            && let Some((key, value)) = tok.split_once('=')
            && is_env_key(key)
        {
            env.push((key.to_string(), value.to_string()));
            continue;
        }
        argv.push(tok);
    }
    if argv.is_empty() {
        return Err(Error::Agent(format!(
            "agent command {program:?} has no program to run"
        )));
    }
    Ok((env, argv))
}

/// A shell-ish environment-assignment key: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whitespace-split honoring single/double quotes (quotes group and are stripped; backslash is
/// a plain character — these are Windows paths, not shell escapes).
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_token = true;
                }
                c if c.is_whitespace() => {
                    if in_token {
                        tokens.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                }
                c => {
                    cur.push(c);
                    in_token = true;
                }
            },
        }
    }
    if in_token {
        tokens.push(cur);
    }
    tokens
}

/// The full argument vector for `repomon-agent-host.exe`, per the PROTOCOL.md §1 spawn
/// contract: `--session S --window W --cwd DIR --owner TOK [--env K=V]... -- PROGRAM ARGS...`.
/// `--cols`/`--rows` are omitted — the host defaults to 220×50 (tmux parity).
pub fn host_spawn_args(
    session: &str,
    window: &str,
    cwd: &str,
    owner: &str,
    env: &[(String, String)],
    argv: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--session".into(),
        session.into(),
        "--window".into(),
        window.into(),
        "--cwd".into(),
        cwd.into(),
        "--owner".into(),
        owner.into(),
    ];
    for (k, v) in env {
        args.push("--env".into());
        args.push(format!("{k}={v}"));
    }
    args.push("--".into());
    args.extend(argv.iter().cloned());
    args
}

/// `session:window` — same shape as the tmux target so clients treat both opaquely.
fn target_of(session: &str, window: &str) -> String {
    format!("{session}:{window}")
}

/// `session:=window` — the exact-match form (tmux parity; the `=` is inert here but keeps the
/// format identical across backends).
fn exact_target_of(session: &str, window: &str) -> String {
    format!("{session}:={window}")
}

/// Recover the window name from a target produced by [`target_of`]/[`exact_target_of`]; a
/// bare window name passes through unchanged.
pub fn window_from_target(session: &str, target: &str) -> String {
    let rest = target
        .strip_prefix(&format!("{session}:"))
        .unwrap_or(target);
    rest.strip_prefix('=').unwrap_or(rest).to_string()
}

/// How a registry-scan connect attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// Pipe connected (hello may or may not have succeeded).
    Connected,
    /// Pipe absent: not found / connection refused — the host is gone.
    Absent,
    /// Pipe exists but every instance was momentarily busy.
    Busy,
}

/// What the scanner does with one registry entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanAction {
    /// Live host owned by us: adopt (list it, drive it).
    Adopt,
    /// Leave it alone (foreign owner, busy pipe, or hello failed on a live pipe).
    Skip,
    /// Stale registry entry: delete the JSON file (PROTOCOL.md §8).
    Gc,
}

/// The adopt/skip/GC rule (PROTOCOL.md §6 + §8): a pipe that won't connect marks the entry
/// stale (GC); a connected host is adopted only when its `hello.owner` matches `me` — on
/// mismatch the daemon MUST back off (not adopt, not reap, not kill). A busy pipe or a failed
/// hello on a live pipe is skipped, never GC'd.
pub fn scan_action(connect: ConnectOutcome, hello_owner: Option<&str>, me: &str) -> ScanAction {
    match connect {
        ConnectOutcome::Absent => ScanAction::Gc,
        ConnectOutcome::Busy => ScanAction::Skip,
        ConnectOutcome::Connected => match hello_owner {
            Some(owner) if owner == me => ScanAction::Adopt,
            _ => ScanAction::Skip,
        },
    }
}

/// Pick the user's interactive shell for plain terminals (`terminal.open`): PowerShell 7
/// (`pwsh`) when installed, else `%COMSPEC%`, else `cmd.exe`.
pub fn user_shell_from(pwsh: Option<std::path::PathBuf>, comspec: Option<String>) -> String {
    if let Some(p) = pwsh {
        return p.to_string_lossy().into_owned();
    }
    comspec
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

/// The command a client runs in a real terminal to attach to `window`: the raw byte-proxy
/// attach client (`repomon attach-host <window>`, Track F).
pub fn attach_command_for(window: &str) -> AttachCommand {
    AttachCommand {
        program: "repomon".to_string(),
        args: vec!["attach-host".to_string(), window.to_string()],
    }
}

/// Whether a host's `program` is the Claude Code CLI, for the liveness probe's per-cwd claude
/// count: basename, extension stripped (`claude`, `claude.cmd`, `C:\…\claude.exe` all match),
/// case-insensitive (Windows filenames).
pub fn is_claude_program(program: &str) -> bool {
    let base = program
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase();
    let stem = base
        .rsplit_once('.')
        .map(|(stem, _ext)| stem)
        .unwrap_or(&base);
    stem == "claude"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_bare_program() {
        let (env, argv) = split_spawn_program("claude").unwrap();
        assert!(env.is_empty());
        assert_eq!(argv, vec!["claude"]);
    }

    #[test]
    fn splits_program_with_args_and_quotes() {
        let (env, argv) =
            split_spawn_program(r#"claude --permission-mode plan --title "my agent""#).unwrap();
        assert!(env.is_empty());
        assert_eq!(
            argv,
            vec!["claude", "--permission-mode", "plan", "--title", "my agent"]
        );
    }

    #[test]
    fn splits_leading_env_assignments_into_env() {
        // The autodetected Claude-variant shape: `CLAUDE_CONFIG_DIR='…' claude`.
        let (env, argv) =
            split_spawn_program(r"CLAUDE_CONFIG_DIR='C:\Users\me\.claude-work' claude").unwrap();
        assert_eq!(
            env,
            vec![(
                "CLAUDE_CONFIG_DIR".to_string(),
                r"C:\Users\me\.claude-work".to_string()
            )]
        );
        assert_eq!(argv, vec!["claude"]);
    }

    #[test]
    fn env_assignment_after_the_program_is_an_argument() {
        let (env, argv) = split_spawn_program("cmd.exe FOO=bar").unwrap();
        assert!(env.is_empty());
        assert_eq!(argv, vec!["cmd.exe", "FOO=bar"]);
    }

    #[test]
    fn backslashes_are_plain_characters() {
        // Windows paths must survive: no shell-style backslash escaping.
        let (_, argv) = split_spawn_program(r"C:\tools\claude.exe --fast").unwrap();
        assert_eq!(argv, vec![r"C:\tools\claude.exe", "--fast"]);
    }

    #[test]
    fn empty_program_is_an_error() {
        assert!(split_spawn_program("").is_err());
        assert!(split_spawn_program("   ").is_err());
        // Only env assignments, nothing to run.
        assert!(split_spawn_program("FOO=bar").is_err());
    }

    #[test]
    fn host_spawn_args_follow_the_protocol_contract() {
        let args = host_spawn_args(
            "repomon",
            "lane-3-1",
            r"C:\work",
            "tok",
            &[("FOO".to_string(), "bar".to_string())],
            &["claude".to_string(), "--permission-mode".to_string(), "plan".to_string()],
        );
        assert_eq!(
            args,
            vec![
                "--session", "repomon", "--window", "lane-3-1", "--cwd", r"C:\work", "--owner",
                "tok", "--env", "FOO=bar", "--", "claude", "--permission-mode", "plan",
            ]
        );
    }

    #[test]
    fn targets_match_the_tmux_shapes_and_round_trip() {
        assert_eq!(target_of("repomon", "lane-7"), "repomon:lane-7");
        assert_eq!(exact_target_of("repomon", "lane-7"), "repomon:=lane-7");
        assert_eq!(window_from_target("repomon", "repomon:lane-7"), "lane-7");
        assert_eq!(window_from_target("repomon", "repomon:=lane-7"), "lane-7");
        // A bare window name passes through (defensive).
        assert_eq!(window_from_target("repomon", "lane-7"), "lane-7");
    }

    #[test]
    fn scan_adopts_own_live_hosts_only() {
        assert_eq!(
            scan_action(ConnectOutcome::Connected, Some("me"), "me"),
            ScanAction::Adopt
        );
        // Foreign owner: back off — never adopt, reap, or kill (PROTOCOL.md §6).
        assert_eq!(
            scan_action(ConnectOutcome::Connected, Some("other"), "me"),
            ScanAction::Skip
        );
        // Live pipe but hello failed: leave it alone, never GC a connectable pipe.
        assert_eq!(
            scan_action(ConnectOutcome::Connected, None, "me"),
            ScanAction::Skip
        );
    }

    #[test]
    fn scan_gcs_only_dead_pipes() {
        assert_eq!(
            scan_action(ConnectOutcome::Absent, None, "me"),
            ScanAction::Gc
        );
        // Busy = alive: never GC, never adopt this pass.
        assert_eq!(
            scan_action(ConnectOutcome::Busy, None, "me"),
            ScanAction::Skip
        );
    }

    #[test]
    fn shell_prefers_pwsh_then_comspec_then_cmd() {
        assert_eq!(
            user_shell_from(
                Some(std::path::PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe")),
                Some(r"C:\Windows\system32\cmd.exe".to_string()),
            ),
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        );
        assert_eq!(
            user_shell_from(None, Some(r"C:\Windows\system32\cmd.exe".to_string())),
            r"C:\Windows\system32\cmd.exe"
        );
        assert_eq!(user_shell_from(None, None), "cmd.exe");
    }

    #[test]
    fn attach_command_runs_the_attach_host_client() {
        let cmd = attach_command_for("lane-7");
        assert_eq!(cmd.program, "repomon");
        assert_eq!(cmd.args, vec!["attach-host", "lane-7"]);
    }

    #[test]
    fn claude_program_matching_handles_paths_and_extensions() {
        assert!(is_claude_program("claude"));
        assert!(is_claude_program("claude.cmd"));
        assert!(is_claude_program("CLAUDE.EXE"));
        assert!(is_claude_program(r"C:\Users\me\AppData\Roaming\npm\claude.cmd"));
        assert!(!is_claude_program("codex"));
        assert!(!is_claude_program("claude-helper.exe"));
        assert!(!is_claude_program(""));
    }
}
