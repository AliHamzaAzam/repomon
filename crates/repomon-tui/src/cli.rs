//! Headless CLI subcommands: `repomon add|remove|discover|lane …|daemon …`.
//!
//! Repo/lane commands talk to the running daemon (the single SQLite writer); daemon
//! commands drive the launchd service in `repomon_core::service`.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use clap::Subcommand;
use repomon_core::model::{Lane, Repo};
use repomon_core::{config, service, Config};
use serde_json::json;

use crate::client::DaemonClient;

#[derive(Subcommand)]
pub enum Command {
    /// Register a repository.
    Add { path: PathBuf },
    /// Unregister a repository (by name or id).
    Remove { repo: String },
    /// Find git repositories under a root.
    Discover {
        root: PathBuf,
        #[arg(long, default_value_t = 4)]
        depth: usize,
        /// Register every repository found.
        #[arg(long)]
        add: bool,
    },
    /// Lane operations.
    Lane {
        #[command(subcommand)]
        cmd: LaneCmd,
    },
    /// Daemon service management.
    Daemon {
        #[command(subcommand)]
        cmd: DaemonCmd,
    },
    /// Remote access for companion apps (iOS): enable the bridge, pair a phone.
    Remote {
        #[command(subcommand)]
        cmd: RemoteCmd,
    },
    /// Talk to repomind — an orchestrator agent that manages the fleet for you. Launches a
    /// `claude` session wired to the repomon MCP server (and your mnemind memory, if present).
    Orchestrate {
        /// How autonomous repomind is: autonomous (default), supervised, or read-only.
        #[arg(long, default_value = "autonomous")]
        autonomy: String,
        /// Cap on how many agents repomind may run at once (default 4).
        #[arg(long)]
        max_agents: Option<usize>,
        /// Override the model for the orchestrator session (e.g. opus, sonnet).
        #[arg(long)]
        model: Option<String>,
        /// An initial goal to start repomind with (optional).
        prompt: Option<String>,
    },
    /// Print a shell completion script to stdout (for eval or install).
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
    /// Print shell integration (cd-on-exit) for `eval "$(repomon shell-init zsh)"`.
    ShellInit {
        /// Shell: zsh, bash, or fish.
        shell: clap_complete::Shell,
    },
    /// Write a roff man page to stdout (used by the Homebrew formula).
    Man,
}

#[derive(Subcommand)]
pub enum LaneCmd {
    /// List all lanes (tab-separated: repo/name, branch, dirty, id).
    List,
    /// Create a lane (worktree) on a branch.
    New {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        branch: String,
        #[arg(long)]
        source: Option<String>,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Delete a lane (by worktree name or id).
    Delete {
        lane: String,
        #[arg(long)]
        delete_branch: bool,
    },
}

#[derive(Subcommand)]
pub enum RemoteCmd {
    /// Turn the WebSocket bridge on: generate a token, detect the Tailscale address, write
    /// the config. Restart the daemon afterwards to apply.
    Enable {
        /// Bind address override (default: the Tailscale IPv4 on port 7878).
        #[arg(long)]
        bind: Option<String>,
        /// Rotate the token even if one already exists.
        #[arg(long)]
        rotate_token: bool,
    },
    /// Show a QR code for the companion app to scan (encodes address + token).
    Pair,
    /// Show the remote-access configuration (token masked).
    Status,
    /// Turn the bridge off (keeps the token for re-enabling).
    Disable,
}

#[derive(Subcommand)]
pub enum DaemonCmd {
    /// Start the daemon if it isn't already running.
    Start,
    /// Stop the running daemon.
    Stop,
    /// Restart the daemon (useful after rebuilding).
    Restart,
    /// Show daemon status.
    Status,
    /// Print the daemon log (tail).
    Logs,
    /// Install + load a launchd-managed service (macOS).
    Install,
    /// Unload + remove the launchd service.
    Uninstall,
}

/// Run a CLI subcommand.
pub async fn handle(cmd: Command, config: &Config, socket: Option<PathBuf>) -> Result<()> {
    match cmd {
        Command::Add { path } => {
            let client = connect(socket, config).await?;
            let repo: Repo = client
                .call_typed("repo.add", Some(json!({ "path": path.to_string_lossy() })))
                .await?;
            println!(
                "added {} ({})  id={}",
                repo.name,
                repo.path.display(),
                repo.id
            );
        }
        Command::Remove { repo } => {
            let client = connect(socket, config).await?;
            let target = resolve_repo(&client, &repo).await?;
            client
                .call("repo.remove", Some(json!({ "repo_id": target.id })))
                .await?;
            println!("removed {} (id={})", target.name, target.id);
        }
        Command::Discover { root, depth, add } => {
            let client = connect(socket, config).await?;
            let paths: Vec<String> = client
                .call_typed(
                    "repo.discover",
                    Some(json!({ "root": root.to_string_lossy(), "max_depth": depth })),
                )
                .await?;
            for p in &paths {
                if add {
                    match client.call("repo.add", Some(json!({ "path": p }))).await {
                        Ok(_) => println!("added   {p}"),
                        Err(e) => println!("skip    {p}  ({e})"),
                    }
                } else {
                    println!("{p}");
                }
            }
            if !add {
                eprintln!(
                    "{} repo(s) found; re-run with --add to register them",
                    paths.len()
                );
            }
        }
        Command::Lane { cmd } => handle_lane(cmd, config, socket).await?,
        Command::Daemon { cmd } => handle_daemon(cmd, config).await?,
        Command::Remote { cmd } => handle_remote(cmd)?,
        Command::Orchestrate {
            autonomy,
            max_agents,
            model,
            prompt,
        } => handle_orchestrate(config, socket, autonomy, max_agents, model, prompt).await?,
        Command::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = crate::Cli::command();
            clap_complete::generate(shell, &mut cmd, "repomon", &mut std::io::stdout());
        }
        Command::ShellInit { shell } => print!("{}", shell_init(shell)?),
        Command::Man => {
            use clap::CommandFactory;
            clap_mangen::Man::new(crate::Cli::command()).render(&mut std::io::stdout())?;
        }
    }
    Ok(())
}

/// `repomon remote …` — manage the companion-app bridge. These edit the config *file* (the
/// token never crosses the RPC surface); the daemon picks changes up on restart.
fn handle_remote(cmd: RemoteCmd) -> Result<()> {
    let path = config::config_path();
    let mut cfg = Config::load().unwrap_or_default();
    match cmd {
        RemoteCmd::Enable { bind, rotate_token } => {
            let bind = match bind.or_else(|| cfg.remote.bind.clone()) {
                Some(b) => b,
                None => {
                    let ip = tailscale_ip().ok_or_else(|| {
                        anyhow!(
                            "couldn't detect a Tailscale IP — is Tailscale running? \
                             (or pass --bind <ip:port> explicitly)"
                        )
                    })?;
                    format!("{ip}:7878")
                }
            };
            if cfg.remote.token.is_none() || rotate_token {
                cfg.remote.token = Some(generate_token());
            }
            cfg.remote.bind = Some(bind.clone());
            cfg.remote.enabled = true;
            cfg.save_to(&path)?;
            println!("remote bridge enabled on ws://{bind}");
            println!("apply with: repomon daemon restart");
            println!("then pair your phone with: repomon remote pair");
        }
        RemoteCmd::Disable => {
            cfg.remote.enabled = false;
            cfg.save_to(&path)?;
            println!("remote bridge disabled (token kept) — repomon daemon restart to apply");
        }
        RemoteCmd::Pair => {
            let (Some(bind), Some(token), true) =
                (&cfg.remote.bind, &cfg.remote.token, cfg.remote.enabled)
            else {
                return Err(anyhow!(
                    "remote access is not enabled — run `repomon remote enable` first"
                ));
            };
            let url = format!("repomon://{bind}#{token}");
            let code = qrcode::QrCode::new(url.as_bytes())?;
            let art = code
                .render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build();
            println!("{art}");
            println!("scan with the repomon iOS app · {url}");
            println!("(anyone with this QR can drive your agents — share it with no one)");
        }
        RemoteCmd::Status => {
            let state = if cfg.remote.enabled {
                "enabled"
            } else {
                "disabled"
            };
            let bind = cfg.remote.bind.as_deref().unwrap_or("(unset)");
            let token = match &cfg.remote.token {
                Some(t) if t.len() >= 8 => format!("{}…{}", &t[..4], &t[t.len() - 4..]),
                Some(_) => "(set)".into(),
                None => "(unset)".into(),
            };
            println!("remote: {state}");
            println!("bind:   ws://{bind}");
            println!("token:  {token}");
            let push_ready = cfg.push.team_id.is_some()
                && cfg.push.key_id.is_some()
                && cfg.push.p8_path.is_some()
                && cfg.push.bundle_id.is_some();
            println!(
                "push:   {}",
                if push_ready {
                    "configured"
                } else {
                    "not configured ([push] in config.toml: team_id, key_id, p8_path, bundle_id)"
                }
            );
        }
    }
    Ok(())
}

/// `repomon orchestrate` — launch the repomind orchestrator: ensure the daemon is up, write an
/// MCP config pointing `claude` at `repomond mcp`, and exec an interactive `claude` session
/// wired to the fleet (and the user's mnemind memory, inherited from their own Claude config).
async fn handle_orchestrate(
    config: &Config,
    socket: Option<PathBuf>,
    autonomy: String,
    max_agents: Option<usize>,
    model: Option<String>,
    prompt: Option<String>,
) -> Result<()> {
    let socket_path = socket.clone().unwrap_or_else(|| config::socket_path(config));
    // Make sure a daemon is running before `repomond mcp` (spawned by claude) tries to connect.
    crate::ensure_daemon(config, socket).await?;

    let repomond = service::repomond_path();

    // The MCP server's environment is authoritative for the socket + guardrails.
    let mut env = serde_json::Map::new();
    env.insert(
        "REPOMON_MCP_SOCKET".into(),
        json!(socket_path.to_string_lossy()),
    );
    env.insert("REPOMON_MCP_AUTONOMY".into(), json!(autonomy));
    if let Some(n) = max_agents {
        env.insert("REPOMON_MCP_MAX_AGENTS".into(), json!(n.to_string()));
    }

    let mcp_config = json!({
        "mcpServers": {
            "repomon": {
                "command": repomond.to_string_lossy(),
                "args": ["mcp"],
                "env": serde_json::Value::Object(env),
            }
        }
    });
    let cfg_dir = config::config_dir();
    std::fs::create_dir_all(&cfg_dir)?;
    let mcp_config_path = cfg_dir.join("repomind-mcp.json");
    std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&mcp_config)?)?;

    // Build the claude invocation. `--mcp-config` *adds* the repomon server; the user's own
    // basic-memory (mnemind) server still loads from their config, so we don't redeclare it.
    let mut cmd = std::process::Command::new("claude");
    cmd.arg("--mcp-config").arg(&mcp_config_path);
    cmd.arg("--append-system-prompt").arg(repomon_mcp::PERSONA);
    // Pre-approve the fleet + memory tools so routine orchestration doesn't prompt; anything
    // else (Bash, file edits) still gates through the normal permission flow.
    cmd.arg("--allowedTools").arg("mcp__repomon,mcp__basic-memory");
    if let Some(model) = &model {
        cmd.arg("--model").arg(model);
    }
    if let Some(prompt) = &prompt {
        cmd.arg(prompt);
    }

    eprintln!("repomind: orchestrating the fleet (autonomy: {autonomy}). Talk to it below.\n");

    // Replace this process with claude so it owns the terminal directly.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // exec only returns on failure (otherwise this process is replaced by claude).
        let err = cmd.exec();
        Err(anyhow!(
            "failed to launch `claude` ({err}). Is Claude Code installed and on PATH?"
        ))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|e| anyhow!("failed to launch `claude` ({e}). Is it installed and on PATH?"))?;
        std::process::exit(status.code().unwrap_or(0));
    }
}

/// A fresh 32-byte hex bearer token from the OS entropy pool (no extra deps).
fn generate_token() -> String {
    let mut buf = [0u8; 32];
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .expect("read /dev/urandom");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The machine's Tailscale IPv4, via the `tailscale` CLI (PATH, then the Mac app bundle).
fn tailscale_ip() -> Option<String> {
    for bin in [
        "tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ] {
        if let Ok(out) = std::process::Command::new(bin).args(["ip", "-4"]).output() {
            if out.status.success() {
                let ip = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !ip.is_empty() {
                    return Some(ip);
                }
            }
        }
    }
    None
}

async fn handle_lane(cmd: LaneCmd, config: &Config, socket: Option<PathBuf>) -> Result<()> {
    let client = connect(socket, config).await?;
    match cmd {
        LaneCmd::List => {
            let lanes: Vec<Lane> = client.call_typed("lane.list", None).await?;
            for l in lanes {
                let name = if l.worktree.is_main {
                    "main".into()
                } else {
                    l.worktree.name.clone()
                };
                let branch = l
                    .state
                    .branch
                    .clone()
                    .unwrap_or_else(|| "(detached)".into());
                let dirty = format!(
                    "+{} ~{} ?{}",
                    l.state.dirty.staged, l.state.dirty.unstaged, l.state.dirty.untracked
                );
                println!(
                    "{}/{}\t{}\t{}\tid={}",
                    l.repo.name, name, branch, dirty, l.id
                );
            }
        }
        LaneCmd::New {
            repo,
            branch,
            source,
            path,
        } => {
            let target = resolve_repo(&client, &repo).await?;
            let params = json!({
                "repo_id": target.id,
                "branch": branch,
                "source_branch": source,
                "path": path.map(|p| p.to_string_lossy().into_owned()),
                "copy_files": [],
            });
            let lane: Lane = client.call_typed("lane.create", Some(params)).await?;
            println!(
                "created lane {} at {}",
                branch,
                lane.worktree.path.display()
            );
        }
        LaneCmd::Delete {
            lane,
            delete_branch,
        } => {
            let lanes: Vec<Lane> = client.call_typed("lane.list", None).await?;
            let target = lanes
                .iter()
                .find(|l| l.id.to_string() == lane || l.worktree.name == lane)
                .ok_or_else(|| anyhow!("no lane matching '{lane}'"))?;
            client
                .call(
                    "lane.delete",
                    Some(json!({ "lane_id": target.id, "also_delete_branch": delete_branch })),
                )
                .await?;
            println!("deleted lane {} (id={})", lane, target.id);
        }
    }
    Ok(())
}

async fn handle_daemon(cmd: DaemonCmd, config: &Config) -> Result<()> {
    let socket = config::socket_path(config);
    match cmd {
        DaemonCmd::Start => {
            crate::ensure_daemon(config, None).await?;
            println!("daemon running (socket: {})", socket.display());
        }
        DaemonCmd::Stop => {
            if stop_running(&socket).await {
                println!("daemon stopped");
            } else {
                println!("no running daemon at {}", socket.display());
            }
            // Also bootout a launchd-managed instance, if any.
            let _ = service::stop();
        }
        DaemonCmd::Restart => {
            stop_running(&socket).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            crate::ensure_daemon(config, None).await?;
            println!("daemon restarted (socket: {})", socket.display());
        }
        DaemonCmd::Status => match DaemonClient::connect(&socket).await {
            Ok(c) => {
                let v = c.call("daemon.status", None).await?;
                println!("running: {v}");
            }
            Err(_) => println!("not running (socket: {})", socket.display()),
        },
        DaemonCmd::Logs => {
            let path = service::log_file();
            match std::fs::read_to_string(&path) {
                Ok(s) => {
                    let lines: Vec<&str> = s.lines().collect();
                    let start = lines.len().saturating_sub(40);
                    for line in &lines[start..] {
                        println!("{line}");
                    }
                }
                Err(_) => println!("no log file yet at {}", path.display()),
            }
        }
        DaemonCmd::Install => {
            service::install(&service::repomond_path(), &socket)?;
            println!("installed and loaded {}", service::plist_path().display());
        }
        DaemonCmd::Uninstall => {
            stop_running(&socket).await;
            service::uninstall()?;
            println!("uninstalled");
        }
    }
    Ok(())
}

/// Tell a running daemon to shut down via the socket (works for an auto-spawned one).
async fn stop_running(socket: &std::path::Path) -> bool {
    match DaemonClient::connect(socket).await {
        Ok(c) => {
            let _ = c.call("daemon.shutdown", None).await;
            true
        }
        Err(_) => false,
    }
}

async fn connect(socket: Option<PathBuf>, config: &Config) -> Result<DaemonClient> {
    // Auto-start a detached daemon if one isn't already running.
    crate::ensure_daemon(config, socket).await
}

async fn resolve_repo(client: &DaemonClient, key: &str) -> Result<Repo> {
    let repos: Vec<Repo> = client.call_typed("repo.list", None).await?;
    repos
        .into_iter()
        .find(|r| r.name == key || r.id.to_string() == key)
        .ok_or_else(|| anyhow!("no repo matching '{key}'"))
}

const POSIX_CD_WRAPPER: &str = r#"# repomon shell integration: cd into a lane's worktree on exit.
repomon() {
  local tmp; tmp=$(mktemp)
  REPOMON_CD_FD=3 command repomon "$@" 3>"$tmp"
  local dir; dir=$(cat "$tmp"); rm -f "$tmp"
  [ -n "$dir" ] && [ -d "$dir" ] && cd "$dir"
}
"#;

const FISH_CD_WRAPPER: &str = r#"# repomon shell integration: cd into a lane's worktree on exit.
function repomon
    set -l tmp (mktemp)
    REPOMON_CD_FD=3 command repomon $argv 3>"$tmp"
    set -l dir (cat "$tmp"); rm -f "$tmp"
    test -n "$dir"; and test -d "$dir"; and cd "$dir"
end
"#;

/// Shell integration snippet (cd-on-exit) for `eval "$(repomon shell-init <shell>)"`.
pub fn shell_init(shell: clap_complete::Shell) -> Result<String> {
    let snippet = match shell {
        clap_complete::Shell::Zsh | clap_complete::Shell::Bash => POSIX_CD_WRAPPER,
        clap_complete::Shell::Fish => FISH_CD_WRAPPER,
        other => {
            return Err(anyhow!(
                "shell-init: unsupported shell '{other}'; use zsh, bash, or fish"
            ))
        }
    };
    Ok(snippet.to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn completions_render_contains_binary_name() {
        use clap::CommandFactory;
        let mut cmd = crate::Cli::command();
        let mut buf = Vec::new();
        clap_complete::generate(clap_complete::Shell::Zsh, &mut cmd, "repomon", &mut buf);
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("repomon"), "completion script should mention repomon");
    }

    #[test]
    fn shell_init_posix_defines_wrapper() {
        let out = super::shell_init(clap_complete::Shell::Zsh).unwrap();
        assert!(out.contains("repomon()"));
        assert!(out.contains("REPOMON_CD_FD=3"));
    }

    #[test]
    fn shell_init_fish_defines_wrapper() {
        let out = super::shell_init(clap_complete::Shell::Fish).unwrap();
        assert!(out.contains("function repomon"));
        assert!(out.contains("REPOMON_CD_FD=3"));
    }

    #[test]
    fn shell_init_rejects_unsupported_shell() {
        assert!(super::shell_init(clap_complete::Shell::PowerShell).is_err());
    }

    #[test]
    fn man_render_contains_binary_name() {
        use clap::CommandFactory;
        let man = clap_mangen::Man::new(crate::Cli::command());
        let mut buf = Vec::new();
        man.render(&mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("repomon"));
    }
}
