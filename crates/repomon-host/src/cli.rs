//! The host's command line — the spawn contract in PROTOCOL.md §1. Parsed with clap on
//! every OS so the contract is tested everywhere, even though only Windows runs it.

use std::path::PathBuf;

use clap::Parser;

/// `repomon-agent-host --session S --window W --cwd DIR [--owner TOK] [--cols N] [--rows N]
/// [--env K=V]... -- PROGRAM [ARGS]...`
#[derive(Debug, Parser)]
#[command(name = "repomon-agent-host", disable_help_flag = false)]
pub struct HostArgs {
    /// Session name (tmux-parity, `[A-Za-z0-9_-]+`).
    #[arg(long)]
    pub session: String,
    /// Window name within the session.
    #[arg(long)]
    pub window: String,
    /// Working directory for the agent child.
    #[arg(long)]
    pub cwd: PathBuf,
    /// Opaque owner token (PROTOCOL.md §6); generated randomly when absent.
    #[arg(long)]
    pub owner: Option<String>,
    /// Initial columns (tmux `new-session -x 220` parity).
    #[arg(long, default_value_t = 220)]
    pub cols: u16,
    /// Initial rows (tmux `new-session -y 50` parity).
    #[arg(long, default_value_t = 50)]
    pub rows: u16,
    /// Extra environment for the child, on top of the host's inherited environment.
    #[arg(long = "env", value_parser = parse_env_pair)]
    pub env: Vec<(String, String)>,
    /// The agent program and its arguments (structured — never a shell string).
    #[arg(last = true, required = true, num_args = 1..)]
    pub command: Vec<String>,
}

fn parse_env_pair(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .ok_or_else(|| format!("expected KEY=VALUE, got {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<HostArgs, clap::Error> {
        HostArgs::try_parse_from(std::iter::once("repomon-agent-host").chain(args.iter().copied()))
    }

    #[test]
    fn full_spawn_line_parses() {
        let a = parse(&[
            "--session",
            "repomon",
            "--window",
            "lane-3-1",
            "--cwd",
            "C:\\work",
            "--owner",
            "tok",
            "--cols",
            "190",
            "--rows",
            "45",
            "--env",
            "FOO=bar",
            "--env",
            "BAZ=qux=quux",
            "--",
            "claude",
            "--permission-mode",
            "plan",
        ])
        .unwrap();
        assert_eq!(a.session, "repomon");
        assert_eq!(a.window, "lane-3-1");
        assert_eq!(a.cwd, std::path::PathBuf::from("C:\\work"));
        assert_eq!(a.owner.as_deref(), Some("tok"));
        assert_eq!((a.cols, a.rows), (190, 45));
        assert_eq!(
            a.env,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux=quux".to_string())
            ],
            "value may contain '='"
        );
        assert_eq!(a.command, vec!["claude", "--permission-mode", "plan"]);
    }

    #[test]
    fn size_defaults_to_tmux_parity_220_by_50() {
        let a = parse(&[
            "--session",
            "s",
            "--window",
            "w",
            "--cwd",
            "/x",
            "--",
            "cmd.exe",
        ])
        .unwrap();
        assert_eq!((a.cols, a.rows), (220, 50));
        assert_eq!(a.owner, None);
        assert!(a.env.is_empty());
    }

    #[test]
    fn command_is_required() {
        assert!(parse(&["--session", "s", "--window", "w", "--cwd", "/x"]).is_err());
        assert!(parse(&["--session", "s", "--window", "w", "--cwd", "/x", "--"]).is_err());
    }

    #[test]
    fn env_without_equals_is_rejected() {
        let r = parse(&[
            "--session",
            "s",
            "--window",
            "w",
            "--cwd",
            "/x",
            "--env",
            "NOEQUALS",
            "--",
            "cmd",
        ]);
        assert!(r.is_err());
    }
}
