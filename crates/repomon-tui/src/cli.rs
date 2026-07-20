//! Headless CLI subcommands: `repomon add|remove|discover|lane …|daemon …`.
//!
//! Repo/lane commands talk to the running daemon (the single SQLite writer); daemon
//! commands drive the login service (launchd or systemd) in `repomon_core::service`.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use clap::Subcommand;
use repomon_core::model::{Lane, Repo};
use repomon_core::{Config, config, service};
use serde_json::{Value, json};

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
    /// Talk to repomind — an orchestrator agent that manages the fleet for you. Launches an
    /// agent session (`claude` by default, `--agent codex` for Codex) wired to the repomon MCP
    /// server (and your mnemind memory, if present).
    Orchestrate {
        /// Which agent powers repomind: a Claude account (e.g. claude-work), a custom agent
        /// name, or codex. Defaults to the `orchestrator_agent` config, then bare claude.
        #[arg(long)]
        agent: Option<String>,
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
    /// Review and approve orchestrator-drafted playbooks (procedural memory).
    Playbooks {
        #[command(subcommand)]
        cmd: PlaybooksCmd,
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
    /// Show a QR code for the companion app to scan (encodes address + token). With `--name`,
    /// mints a named, individually-revocable per-device token via the daemon instead of encoding
    /// the shared config token.
    Pair {
        /// Pair this named device with its own revocable token (via the daemon).
        #[arg(long)]
        name: Option<String>,
    },
    /// List paired remote devices (name, role, created, last-seen).
    Devices,
    /// Revoke a paired device's token by name.
    Revoke { name: String },
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
    /// Install + load the login service (launchd on macOS, systemd user unit on Linux).
    Install,
    /// Unload + remove the login service.
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
        Command::Daemon { cmd } => handle_daemon(cmd, config, socket).await?,
        Command::Remote { cmd } => handle_remote(cmd, config, socket).await?,
        Command::Playbooks { cmd } => handle_playbooks(cmd, config, socket).await?,
        Command::Orchestrate {
            agent,
            autonomy,
            max_agents,
            model,
            prompt,
        } => handle_orchestrate(config, socket, agent, autonomy, max_agents, model, prompt).await?,
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

/// `repomon remote …` — manage the companion-app bridge. Enable/Disable/Status and an un-named
/// Pair edit the config *file* (the shared token never crosses the RPC surface); the daemon picks
/// those up on restart. The per-device flows (`pair --name`, `devices`, `revoke`) instead talk to
/// the running daemon over the local socket, since device tokens live in the store.
async fn handle_remote(cmd: RemoteCmd, config: &Config, socket: Option<PathBuf>) -> Result<()> {
    match cmd {
        RemoteCmd::Pair { name: Some(name) } => remote_pair_named(config, socket, name).await,
        RemoteCmd::Devices => remote_devices(config, socket).await,
        RemoteCmd::Revoke { name } => remote_revoke(config, socket, name).await,
        // Enable/Disable/Status and un-named Pair keep editing the config file directly.
        other => handle_remote_config(other),
    }
}

/// The config-file half of `repomon remote …` (Enable/Disable/Status, and an un-named Pair).
fn handle_remote_config(cmd: RemoteCmd) -> Result<()> {
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
        RemoteCmd::Pair { name: _ } => {
            // Only the un-named Pair reaches here (the `--name` case is daemon-backed above).
            let (Some(bind), Some(token), true) =
                (&cfg.remote.bind, &cfg.remote.token, cfg.remote.enabled)
            else {
                return Err(anyhow!(
                    "remote access is not enabled — run `repomon remote enable` first"
                ));
            };
            let url = format!("repomon://{bind}#{token}");
            render_pair_qr(&url)?;
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
        // Routed to the daemon-backed handlers by `handle_remote`; never reach the config path.
        RemoteCmd::Devices | RemoteCmd::Revoke { .. } => unreachable!(),
    }
    Ok(())
}

/// Render a pairing URL as a scannable QR plus the URL and the sharing warning. Shared by the
/// legacy config-token pair and the per-device `pair --name` flow so both print identically.
fn render_pair_qr(url: &str) -> Result<()> {
    let code = qrcode::QrCode::new(url.as_bytes())?;
    let art = code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build();
    println!("{art}");
    println!("scan with the repomon iOS app · {url}");
    println!("(anyone with this QR can drive your agents — share it with no one)");
    Ok(())
}

/// `repomon remote pair --name <device>` — mint (or re-show) this device's own revocable token via
/// the daemon and print its QR.
async fn remote_pair_named(config: &Config, socket: Option<PathBuf>, name: String) -> Result<()> {
    let client = crate::ensure_daemon(config, socket).await?;
    let resp = client
        .call("remote.pair", Some(json!({ "name": name })))
        .await?;
    let url = resp
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("daemon returned no pairing url"))?;
    render_pair_qr(url)?;
    println!("paired device '{name}'. revoke with `repomon remote revoke {name}`");
    Ok(())
}

/// `repomon remote devices` — list paired devices in aligned rows, then the legacy shared token.
async fn remote_devices(config: &Config, socket: Option<PathBuf>) -> Result<()> {
    let client = crate::ensure_daemon(config, socket).await?;
    let resp = client.call("remote.devices", None).await?;
    let devices = resp.as_array().cloned().unwrap_or_default();
    if devices.is_empty() {
        println!("no paired devices");
    } else {
        let str_field =
            |d: &Value, k: &str| d.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let name_w = devices
            .iter()
            .map(|d| str_field(d, "name").len())
            .max()
            .unwrap_or(4)
            .max(4);
        for d in &devices {
            let name = str_field(d, "name");
            let role = str_field(d, "role");
            let created = str_field(d, "created_at");
            let seen = d
                .get("last_seen_at")
                .and_then(|v| v.as_str())
                .unwrap_or("never");
            println!("{name:<name_w$}  {role:<6}  created {created}  seen {seen}");
        }
    }
    // The shared config token (if configured) isn't a listed device — call it out so it isn't
    // mistaken for gone. It's retired by rotating it: `repomon remote enable --rotate-token`.
    if config.remote.token.is_some() {
        println!("(config token - shared; repomon remote enable --rotate-token to retire)");
    }
    Ok(())
}

/// `repomon remote revoke <name>` — revoke a device's token via the daemon.
async fn remote_revoke(config: &Config, socket: Option<PathBuf>, name: String) -> Result<()> {
    let client = crate::ensure_daemon(config, socket).await?;
    let resp = client
        .call("remote.revoke", Some(json!({ "name": name })))
        .await?;
    let revoked = resp
        .get("revoked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if revoked {
        println!("revoked device '{name}'; its token no longer connects");
    } else {
        println!("no paired device named '{name}'");
    }
    Ok(())
}

/// `repomon orchestrate` — talk to the repomind orchestrator. Ensure the daemon is up, ask it to
/// start (or reuse) the single daemon-owned orchestrator session, then `tmux attach` to that
/// durable window. The session-building (MCP config + `claude` invocation) now lives daemon-side
/// in `orchestrator.start`, so the CLI and the TUI drive **one** shared orchestrator.
#[allow(clippy::too_many_arguments)]
async fn handle_orchestrate(
    config: &Config,
    socket: Option<PathBuf>,
    agent: Option<String>,
    autonomy: String,
    max_agents: Option<usize>,
    model: Option<String>,
    prompt: Option<String>,
) -> Result<()> {
    // Make sure a daemon is running, then drive it (it owns the orchestrator window).
    let client = crate::ensure_daemon(config, socket).await?;

    // `orchestrator.start` below is idempotent — a no-op if a session is already running (e.g.
    // the TUI auto-started repomind at its own default autonomy when the command-center opened).
    // Check first so we never assert an autonomy that isn't actually in force: only print the
    // "starting at {autonomy}" banner when this call is the one that actually launches it.
    let status = client
        .call("orchestrator.status", None)
        .await
        .map_err(|e| anyhow!("failed to query the orchestrator: {e}"))?;
    let already_running = status
        .get("running")
        .and_then(|r| r.as_bool())
        .unwrap_or(false);
    if already_running {
        let actual = status
            .get("autonomy")
            .and_then(|a| a.as_str())
            .map(|a| a.to_string())
            .unwrap_or_else(|| "unknown (adopted session)".to_string());
        eprintln!(
            "repomind is already running (autonomy: {actual}) — attaching. Stop it first (orchestrator.stop / TUI) to relaunch with different settings.\n"
        );
    } else {
        eprintln!("repomind: orchestrating the fleet (autonomy: {autonomy}). Talk to it below.\n");
    }

    // Start (or adopt) the orchestrator session. Idempotent: a no-op if one is already running.
    let mut start = serde_json::Map::new();
    start.insert("autonomy".into(), json!(autonomy));
    if let Some(agent) = &agent {
        start.insert("agent".into(), json!(agent));
    }
    if let Some(model) = &model {
        start.insert("model".into(), json!(model));
    }
    if let Some(n) = max_agents {
        start.insert("max_agents".into(), json!(n));
    }
    if let Some(prompt) = &prompt {
        start.insert("prompt".into(), json!(prompt));
    }
    client
        .call("orchestrator.start", Some(Value::Object(start)))
        .await
        .map_err(|e| anyhow!("failed to start the orchestrator: {e}"))?;

    // Resolve its attach target and attach to the durable tmux window.
    let resp = client.call("orchestrator.target", None).await?;
    let target = resp
        .get("target")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let available = resp
        .get("available")
        .and_then(|a| a.as_bool())
        .unwrap_or(false);
    if !available || target.is_empty() {
        return Err(anyhow!(
            "the orchestrator session isn't available — is tmux installed and on PATH?"
        ));
    }

    attach_tmux_target(&target)
}

/// Attach this process to a `session:window` target on repomon's dedicated tmux socket (the socket
/// label is the session name). `$TMUX` is dropped so this works even from inside tmux. On unix we
/// `exec` tmux so it owns the terminal directly (like a raw attach); detaching ends the command.
fn attach_tmux_target(target: &str) -> Result<()> {
    let socket_label = target.split(':').next().unwrap_or("repomon");
    let mut cmd = std::process::Command::new("tmux");
    cmd.args(["-L", socket_label, "attach", "-t", target])
        .env_remove("TMUX");
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // exec only returns on failure (otherwise this process is replaced by tmux).
        let err = cmd.exec();
        Err(anyhow!(
            "failed to attach to the orchestrator ({err}). Is tmux installed and on PATH?"
        ))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .map_err(|e| anyhow!("failed to attach to the orchestrator ({e})."))?;
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

#[derive(Subcommand)]
pub enum PlaybooksCmd {
    /// List all playbooks (name, status, updated, pending revision marker).
    List,
    /// Print a playbook's content (and its pending revision, if any).
    Show { name: String },
    /// Approve a draft (or promote an approved playbook's pending revision).
    Approve { name: String },
    /// Delete a playbook outright.
    Delete { name: String },
}

/// `repomon playbooks ...` — the human approval surface for orchestrator-drafted playbooks.
/// Drafts are inert until approved here (or via the daemon RPC this drives).
async fn handle_playbooks(
    cmd: PlaybooksCmd,
    config: &Config,
    socket: Option<PathBuf>,
) -> Result<()> {
    let client = connect(socket, config).await?;
    match cmd {
        PlaybooksCmd::List => {
            let res = client.call("playbook.list", None).await?;
            let books = res
                .get("playbooks")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if books.is_empty() {
                println!("no playbooks yet (repomind drafts them after completed goals)");
                return Ok(());
            }
            for b in books {
                let name = b["name"].as_str().unwrap_or("?");
                let status = b["status"].as_str().unwrap_or("?");
                let updated = b["updated_at"].as_str().unwrap_or("?");
                let pending = if b["draft_content"].is_string() {
                    "  (pending revision)"
                } else {
                    ""
                };
                println!("{name}	{status}	{updated}{pending}");
            }
        }
        PlaybooksCmd::Show { name } => {
            let res = client.call("playbook.list", None).await?;
            let books = res
                .get("playbooks")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let Some(b) = books.iter().find(|b| b["name"].as_str() == Some(&*name)) else {
                anyhow::bail!("no playbook named {name:?} (see `repomon playbooks list`)");
            };
            println!(
                "# {} [{}]\n\n{}",
                name,
                b["status"].as_str().unwrap_or("?"),
                b["content"].as_str().unwrap_or("")
            );
            if let Some(rev) = b["draft_content"].as_str() {
                println!("\n--- pending revision (approve to promote) ---\n\n{rev}");
            }
        }
        PlaybooksCmd::Approve { name } => {
            client
                .call("playbook.approve", Some(json!({ "name": name })))
                .await?;
            println!("approved playbook {name} (repomind will follow it from the next search)");
        }
        PlaybooksCmd::Delete { name } => {
            client
                .call("playbook.delete", Some(json!({ "name": name })))
                .await?;
            println!("deleted playbook {name}");
        }
    }
    Ok(())
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

/// The socket a `daemon` subcommand should target: the CLI `--socket` flag when given, else
/// the config's. (The same precedence `ensure_daemon` applies for every other subcommand.)
fn daemon_socket(flag: Option<PathBuf>, config: &Config) -> PathBuf {
    flag.unwrap_or_else(|| config::socket_path(config))
}

async fn handle_daemon(
    cmd: DaemonCmd,
    config: &Config,
    socket_flag: Option<PathBuf>,
) -> Result<()> {
    let explicit_socket = socket_flag.is_some();
    let socket = daemon_socket(socket_flag, config);
    match cmd {
        DaemonCmd::Start => {
            crate::ensure_daemon(config, Some(socket.clone())).await?;
            println!("daemon running (socket: {})", socket.display());
        }
        DaemonCmd::Stop => {
            if stop_running(&socket).await {
                println!("daemon stopped");
            } else {
                println!("no running daemon at {}", socket.display());
            }
            // Also stop a service-managed instance (launchd/systemd), if any — but only when
            // targeting the default socket. An explicit `--socket` means an isolated daemon;
            // unloading the service would take down the real fleet alongside it.
            if !explicit_socket {
                let _ = service::stop();
            }
        }
        DaemonCmd::Restart => {
            stop_running(&socket).await;
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            crate::ensure_daemon(config, Some(socket.clone())).await?;
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
            println!(
                "installed and loaded {}",
                service::service_file_path().display()
            );
            #[cfg(target_os = "linux")]
            println!("tip: run `loginctl enable-linger` so repomond survives logout");
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
            ));
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
        assert!(
            out.contains("repomon"),
            "completion script should mention repomon"
        );
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

    #[test]
    fn daemon_socket_prefers_the_cli_flag() {
        use repomon_core::Config;
        use std::path::PathBuf;

        // The regression this guards: `repomon --socket X daemon stop|status|restart` used to
        // resolve the socket from config alone and hit the DEFAULT daemon — stopping the real
        // fleet daemon when the caller meant an isolated one.
        let config = Config {
            socket_path: Some(PathBuf::from("/tmp/from-config.sock")),
            ..Default::default()
        };
        assert_eq!(
            super::daemon_socket(Some(PathBuf::from("/tmp/from-flag.sock")), &config),
            PathBuf::from("/tmp/from-flag.sock"),
            "an explicit --socket must always win"
        );
        assert_eq!(
            super::daemon_socket(None, &config),
            PathBuf::from("/tmp/from-config.sock"),
            "without the flag, the config path applies as before"
        );
    }
}
