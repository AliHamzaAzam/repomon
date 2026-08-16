//! Headless CLI subcommands: `repomon add|remove|discover|lane …|daemon …`.
//!
//! Repo/lane commands talk to the running daemon (the single SQLite writer); daemon
//! commands drive the login service (launchd or systemd) in `repomon_core::service`.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use chrono::Utc;
use clap::Subcommand;
use repomon_core::model::{
    AgentChoice, FleetMessage, Lane, LaneId, MessagePage, Repo, TranscriptItem,
};
use repomon_core::{Config, config, service};
use repomon_mcp::fleet::{self, Attention};
use repomon_mcp::server::{approve_key, target_window};
use serde_json::{Value, json};

use crate::client::DaemonClient;

// Lives in `src/attach_client.rs` (declared here to keep `lib.rs` untouched — it is a
// cross-track conflict hotspot during the native-Windows waves).
#[path = "attach_client.rs"]
pub mod attach_client;

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
    /// Durable fleet mail.
    Msg {
        #[command(subcommand)]
        cmd: MsgCmd,
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
        /// Instead of launching a session, register a standing schedule for this prompt (e.g.
        /// "weekdays 09:00", "daily 21:00", "every 2h"). The daemon then runs it headless,
        /// bounded, and delivers the result as a notification.
        #[arg(long)]
        schedule: Option<String>,
        /// Action cap for the scheduled standing run (default 10, max 50).
        #[arg(long)]
        max_actions: Option<u32>,
        /// Override the model for the orchestrator session (e.g. opus, sonnet).
        #[arg(long)]
        model: Option<String>,
        /// An initial goal to start repomind with (optional).
        prompt: Option<String>,
    },
    /// (Windows) Attach this console to an agent host window — raw proxy; F12 detaches.
    AttachHost { window: String },
    /// List or remove standing-orchestration schedules (see `orchestrate --schedule`).
    Schedules {
        #[command(subcommand)]
        cmd: SchedulesCmd,
    },
    /// Manage the per-repo approval allowlist (patterns the daemon auto-approves).
    Approvals {
        #[command(subcommand)]
        cmd: ApprovalsCmd,
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
        /// Shell: zsh, bash, fish, or powershell.
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
    /// Spawn an agent into a lane with a task (mirrors the MCP `spawn_agent` tool).
    Spawn {
        /// The lane (worktree) id to work in.
        #[arg(long)]
        lane: LaneId,
        /// Agent kind/name (e.g. claude-code, codex). Defaults to the configured default agent.
        #[arg(long)]
        agent: Option<String>,
        /// The task prompt. Use "-" or omit (when piped) to read the prompt from stdin.
        #[arg(long)]
        task: Option<String>,
        /// Launch/permission mode (translated per agent kind; `default` emits nothing).
        #[arg(long, value_enum, default_value_t = SpawnMode::Default)]
        mode: SpawnMode,
        /// Model override forwarded to the agent (e.g. opus).
        #[arg(long)]
        model: Option<String>,
        /// Reasoning effort, translated per agent kind. Claude: low|medium|high|xhigh|max|ultracode;
        /// codex: low|medium|high (higher levels clamp to high).
        #[arg(long)]
        effort: Option<String>,
        /// Spawn even if the lane already has a live managed agent (otherwise refused, to avoid
        /// putting two agents in one worktree).
        #[arg(long)]
        force: bool,
    },
    /// Type an instruction into a lane's agent and submit it (mirrors MCP `send_to_agent`).
    Send {
        /// The lane (worktree) id to send to.
        #[arg(long)]
        lane: LaneId,
        /// The text to send. Use "-" or omit (when piped) to read it from stdin.
        #[arg(long)]
        text: Option<String>,
        /// Insert the text without pressing Enter (e.g. to paste a path).
        #[arg(long)]
        no_submit: bool,
        /// Target a specific agent window in a multi-agent lane (default: the primary session).
        #[arg(long)]
        window: Option<String>,
    },
    /// Answer a pending permission/decision dialog (mirrors MCP `approve_agent`).
    Approve {
        /// The lane (worktree) id to answer.
        #[arg(long)]
        lane: LaneId,
        /// "yes" (default), "no", or an option number.
        #[arg(long)]
        choice: Option<String>,
        /// Target a specific agent window in a multi-agent lane (default: the primary session).
        #[arg(long)]
        window: Option<String>,
    },
    /// Interrupt a lane's agent: Escape (soft) by default, or Ctrl-C with --hard (mirrors
    /// MCP `interrupt_agent`).
    Interrupt {
        /// The lane (worktree) id to interrupt.
        #[arg(long)]
        lane: LaneId,
        /// Send Ctrl-C instead of Escape.
        #[arg(long)]
        hard: bool,
        /// Target a specific agent window in a multi-agent lane (default: the lane's first slot).
        #[arg(long)]
        window: Option<String>,
    },
    /// Read a lane's agent: status, attention, the open dialog, and a transcript tail
    /// (mirrors MCP `read_agent`).
    Read {
        /// The lane (worktree) id to read.
        #[arg(long)]
        lane: LaneId,
        /// How many transcript items to show (default 12).
        #[arg(long, default_value_t = 12)]
        transcript_limit: usize,
    },
}

#[derive(Subcommand)]
pub enum MsgCmd {
    /// Send durable mail as the human operator.
    Send {
        /// Canonical recipient such as lane-7/2, @reviewer, or repomind.
        address: String,
        /// Full message body, capped at 8 KiB by the daemon.
        body: String,
        /// Reply to an existing message and inherit its thread budget.
        #[arg(long)]
        reply_to: Option<String>,
    },
    /// List durable fleet mail newest first.
    List {
        /// Show only unread messages.
        #[arg(long)]
        unread: bool,
        /// Filter by recipient lane.
        #[arg(long)]
        lane: Option<LaneId>,
        /// Maximum rows, capped by the daemon.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

/// `lane spawn --mode`: a constrained launch mode so clap rejects bad values at parse time. The
/// daemon translates it per agent kind; `Default` is sent as `"default"` and emits no flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SpawnMode {
    Default,
    Auto,
    Plan,
}

impl SpawnMode {
    /// The lowercase wire value forwarded to the daemon's `agent.spawn`.
    fn as_wire(self) -> &'static str {
        match self {
            SpawnMode::Default => "default",
            SpawnMode::Auto => "auto",
            SpawnMode::Plan => "plan",
        }
    }
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
        Command::Msg { cmd } => handle_msg(cmd, config, socket).await?,
        Command::Daemon { cmd } => handle_daemon(cmd, config, socket).await?,
        Command::Remote { cmd } => handle_remote(cmd, config, socket).await?,
        Command::Playbooks { cmd } => handle_playbooks(cmd, config, socket).await?,
        Command::Schedules { cmd } => handle_schedules(cmd, config, socket).await?,
        Command::Approvals { cmd } => handle_approvals(cmd, config, socket).await?,
        Command::Orchestrate {
            agent,
            autonomy,
            max_agents,
            schedule,
            max_actions,
            model,
            prompt,
        } => {
            if let Some(spec) = schedule {
                handle_schedule_add(config, socket, spec, max_actions, prompt).await?
            } else {
                handle_orchestrate(config, socket, agent, autonomy, max_agents, model, prompt)
                    .await?
            }
        }
        Command::AttachHost { window } => attach_client::run(&config.tmux_session, &window).await?,
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

    attach_tmux_target(&target, resp.get("attach"))
}

/// Attach this process to a `session:window` target. Prefers the daemon-provided attach command
/// (the optional `attach` response field: `{ program, args }`); falls back to the classic tmux
/// invocation on repomon's dedicated socket (the socket label is the session name — the target's
/// prefix) for daemons without the field. `$TMUX` is dropped so this works even from inside tmux.
/// On unix we `exec` the attach program so it owns the terminal directly (like a raw attach);
/// detaching ends the command.
fn attach_tmux_target(target: &str, attach: Option<&Value>) -> Result<()> {
    let parsed = attach.and_then(|a| {
        let program = a.get("program")?.as_str()?.to_string();
        let args = a
            .get("args")?
            .as_array()?
            .iter()
            .map(|s| Some(s.as_str()?.to_string()))
            .collect::<Option<Vec<_>>>()?;
        Some((program, args))
    });
    let (program, args) = parsed.unwrap_or_else(|| {
        let socket_label = target.split(':').next().unwrap_or("repomon");
        (
            repomon_core::agent::tmux_program()
                .to_string_lossy()
                .into_owned(),
            vec![
                "-L".to_string(),
                socket_label.to_string(),
                "attach".to_string(),
                "-t".to_string(),
                target.to_string(),
            ],
        )
    });
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).env_remove("TMUX");
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

/// A fresh 32-byte hex bearer token from the OS entropy pool (`getrandom`, portable across
/// unix and Windows).
fn generate_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS entropy source");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Candidate `tailscale` binaries: PATH first, then the platform's default install
/// location (the Mac app bundle, or the Windows Program Files directory).
fn tailscale_candidates() -> [&'static str; 2] {
    let fallback = if cfg!(windows) {
        r"C:\Program Files\Tailscale\tailscale.exe"
    } else {
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
    };
    ["tailscale", fallback]
}

/// The machine's Tailscale IPv4, via the `tailscale` CLI.
fn tailscale_ip() -> Option<String> {
    for bin in tailscale_candidates() {
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
pub enum SchedulesCmd {
    /// List schedules (id, spec, next run, prompt).
    List,
    /// Remove a schedule by id.
    Remove { id: i64 },
}

/// `repomon orchestrate --schedule <spec> "<prompt>"` — register a standing run.
async fn handle_schedule_add(
    config: &Config,
    socket: Option<PathBuf>,
    spec: String,
    max_actions: Option<u32>,
    prompt: Option<String>,
) -> Result<()> {
    let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) else {
        anyhow::bail!(
            "--schedule needs a prompt, e.g. repomon orchestrate --schedule \"weekdays 09:00\" \"morning fleet briefing\""
        );
    };
    let client = connect(socket, config).await?;
    let res = client
        .call(
            "schedule.add",
            Some(json!({ "spec": spec, "prompt": prompt, "max_actions": max_actions })),
        )
        .await?;
    println!(
        "scheduled #{} — {} (next run {})",
        res["id"],
        res["spec"].as_str().unwrap_or("?"),
        res["next_run"].as_str().unwrap_or("?")
    );
    println!("the daemon runs it headless and bounded; results arrive as notifications");
    Ok(())
}

async fn handle_schedules(
    cmd: SchedulesCmd,
    config: &Config,
    socket: Option<PathBuf>,
) -> Result<()> {
    let client = connect(socket, config).await?;
    match cmd {
        SchedulesCmd::List => {
            let res = client.call("schedule.list", None).await?;
            let scheds = res
                .get("schedules")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if scheds.is_empty() {
                println!(
                    "no schedules (add one with: repomon orchestrate --schedule \"weekdays 09:00\" \"morning fleet briefing\")"
                );
                return Ok(());
            }
            for sc in scheds {
                println!(
                    "#{}\t{}\tnext {}\t{}",
                    sc["id"],
                    sc["spec"].as_str().unwrap_or("?"),
                    sc["next_run"].as_str().unwrap_or("?"),
                    sc["prompt"].as_str().unwrap_or("")
                );
            }
        }
        SchedulesCmd::Remove { id } => {
            client
                .call("schedule.remove", Some(json!({ "id": id })))
                .await?;
            println!("removed schedule #{id}");
        }
    }
    Ok(())
}

#[derive(Subcommand)]
pub enum ApprovalsCmd {
    /// List confirmed approval rules (repo, pattern, since).
    List,
    /// Allowlist a command pattern for a repo (the daemon then auto-approves matches).
    Allow { repo: String, pattern: String },
    /// Remove an approval rule.
    Remove { repo: String, pattern: String },
}

/// `repomon approvals ...` — the CLI surface over the approval allowlist. The same rules feed
/// the daemon's auto-approve; force-push/rm -rf/reset --hard always escalate regardless.
async fn handle_approvals(
    cmd: ApprovalsCmd,
    config: &Config,
    socket: Option<PathBuf>,
) -> Result<()> {
    let client = connect(socket, config).await?;
    match cmd {
        ApprovalsCmd::List => {
            let res = client.call("approval.list", None).await?;
            let rules = res
                .get("rules")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if rules.is_empty() {
                println!("no approval rules (repomind proposes them after 3 consistent approvals)");
                return Ok(());
            }
            for r in rules {
                println!(
                    "{}\t{}\tsince {}",
                    r["repo"].as_str().unwrap_or("?"),
                    r["pattern"].as_str().unwrap_or("?"),
                    r["created_at"].as_str().unwrap_or("?")
                );
            }
        }
        ApprovalsCmd::Allow { repo, pattern } => {
            client
                .call(
                    "approval.allow",
                    Some(json!({ "repo": repo, "pattern": pattern })),
                )
                .await?;
            println!(
                "allowlisted '{pattern}' in {repo} — the daemon now auto-approves matching \
                 Bash permissions (destructive commands still always escalate)"
            );
        }
        ApprovalsCmd::Remove { repo, pattern } => {
            client
                .call(
                    "approval.remove",
                    Some(json!({ "repo": repo, "pattern": pattern })),
                )
                .await?;
            println!("removed approval rule '{pattern}' in {repo}");
        }
    }
    Ok(())
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

async fn handle_msg(cmd: MsgCmd, config: &Config, socket: Option<PathBuf>) -> Result<()> {
    let client = connect(socket, config).await?;
    match cmd {
        MsgCmd::Send {
            address,
            body,
            reply_to,
        } => {
            let message: FleetMessage = client
                .call_typed(
                    "message.send",
                    Some(json!({ "to": address, "body": body, "reply_to": reply_to })),
                )
                .await?;
            println!(
                "sent {}  {} -> {}  thread={} hops={}",
                message.id,
                message.sender.address,
                message.recipient.address,
                message.thread_id,
                message.remaining_hops
            );
        }
        MsgCmd::List {
            unread,
            lane,
            limit,
        } => {
            let page: MessagePage = client
                .call_typed(
                    "message.list",
                    Some(json!({
                        "lane_id": lane,
                        "unread_only": unread,
                        "limit": limit,
                    })),
                )
                .await?;
            if page.messages.is_empty() {
                println!("no fleet messages");
                return Ok(());
            }
            for message in page.messages {
                let body = message
                    .body
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let body: String = body.chars().take(160).collect();
                println!(
                    "{}\t{}\t{} -> {}\t{:?}/{:?}\t{}",
                    message.created_at.to_rfc3339(),
                    message.id,
                    message.sender.address,
                    message.recipient.address,
                    message.delivery_state,
                    message.read_state,
                    body
                );
            }
            if let Some(cursor) = page.next_before {
                eprintln!("more messages available before {cursor}");
            }
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
        LaneCmd::Spawn {
            lane,
            agent,
            task,
            mode,
            model,
            effort,
            force,
        } => {
            // Verb-level duplicate-agent guard: refuse to spawn into a lane that already has a live
            // managed agent (which would put two agents in one worktree), unless --force. This is
            // intentionally NOT enforced daemon-side — the TUI multi-spawns a lane on purpose.
            if !force {
                let target: Lane = lane_get(&client, lane).await?;
                if let Some(live) = fleet::live_managed_agent(&target) {
                    let window = live.tmux_window.as_deref().unwrap_or("?");
                    return Err(anyhow!(
                        "lane {lane} already has a live agent (window {window}); spawning another \
                         would put two agents in one worktree. Re-run with --force to spawn anyway, \
                         or use `lane send`/`lane read` to drive the existing one."
                    ));
                }
            }
            // Mirror the MCP `spawn_agent` tool: resolve the agent (configured default when
            // omitted) and issue the same `agent.spawn` daemon request. The daemon translates
            // mode/model/effort per agent kind (`default` mode emits nothing).
            let task = read_task(task)?;
            let agent = match agent {
                Some(name) => name,
                None => default_agent(&client).await,
            };
            let resp = client
                .call(
                    "agent.spawn",
                    Some(json!({
                        "lane_id": lane,
                        "agent": agent,
                        "task": task,
                        "mode": mode.as_wire(),
                        "model": model,
                        "effort": effort,
                    })),
                )
                .await?;
            // The daemon echoes back the tmux window it launched the agent into.
            match resp.get("window").and_then(Value::as_str) {
                Some(window) => println!("spawned {agent} in lane {lane} (window {window})"),
                None => println!("spawned {agent} in lane {lane}"),
            }
        }
        LaneCmd::Send {
            lane,
            text,
            no_submit,
            window,
        } => {
            // Mirror MCP `send_to_agent`: resolve the primary window (reusing target_window's
            // external-session refusal) and issue the same `agent.send_input` request.
            let text = read_task(text)?.ok_or_else(|| {
                anyhow!("no text to send — pass --text <s>, --text -, or pipe it on stdin")
            })?;
            let target: Lane = lane_get(&client, lane).await?;
            let window =
                target_window(fleet::primary_agent(&target), window).map_err(|e| anyhow!(e))?;
            client
                .call(
                    "agent.send_input",
                    Some(json!({
                        "lane_id": lane,
                        "text": text,
                        "enter": !no_submit,
                        "window": window,
                    })),
                )
                .await?;
            println!("sent to lane {lane} (window {window})");
        }
        LaneCmd::Approve {
            lane,
            choice,
            window,
        } => {
            // Mirror MCP `approve_agent`, but: a human at the CLI legitimately answers decisions,
            // so we only WARN (never refuse) when the lane isn't on a routine permission.
            let target: Lane = lane_get(&client, lane).await?;
            let primary = fleet::primary_agent(&target);
            let attention = primary
                .map(fleet::agent_attention)
                .unwrap_or(Attention::None);
            match attention {
                Attention::Permission => {}
                Attention::Decision => eprintln!(
                    "warning: this lane is on a DECISION, not a routine permission — make sure \
                     you mean to answer it for the human."
                ),
                Attention::EndOfTurn | Attention::DoneCandidate => eprintln!(
                    "warning: the agent ended its turn (no open dialog) — your keypress will go \
                     to the prompt. Consider `lane send` instead."
                ),
                Attention::None => {
                    eprintln!("warning: no pending dialog detected on this lane right now.")
                }
            }
            let window = target_window(primary, window).map_err(|e| anyhow!(e))?;
            let choice = choice.map(Value::String);
            let (key, answered) = approve_key(choice.as_ref()).map_err(|e| anyhow!(e))?;
            client
                .call(
                    "agent.key",
                    Some(json!({ "lane_id": lane, "key": key, "window": window })),
                )
                .await?;
            println!("answered {answered} on lane {lane} (sent {key})");
        }
        LaneCmd::Interrupt { lane, hard, window } => {
            // Mirror MCP `interrupt_agent`: soft = Escape via agent.key, --hard = C-c via
            // agent.signal. `window` is optional (the daemon targets the lane's first slot).
            if hard {
                client
                    .call(
                        "agent.signal",
                        Some(json!({ "lane_id": lane, "key": "C-c", "window": window })),
                    )
                    .await?;
                println!("interrupted lane {lane} (hard, C-c)");
            } else {
                client
                    .call(
                        "agent.key",
                        Some(json!({ "lane_id": lane, "key": "Escape", "window": window })),
                    )
                    .await?;
                println!("interrupted lane {lane} (Escape)");
            }
        }
        LaneCmd::Read {
            lane,
            transcript_limit,
        } => {
            // Mirror MCP `read_agent`: project the lane and print a compact transcript tail,
            // reusing the same fleet helpers so the CLI and MCP report identical state.
            let target: Lane = lane_get(&client, lane).await?;
            let digest = fleet::project_lane(&target, Utc::now());
            let primary = fleet::primary_agent(&target);
            let session_id = primary.and_then(|s| s.session_id.clone());
            let pending_prompt = primary.and_then(|s| s.pending_prompt.clone());
            let transcript: Vec<TranscriptItem> = client
                .call_typed(
                    "agent.transcript",
                    Some(json!({
                        "lane_id": lane,
                        "limit": transcript_limit,
                        "session_id": session_id,
                    })),
                )
                .await
                .unwrap_or_default();

            println!("lane {lane}  {}/{}", digest.repo, digest.branch);
            println!("dirty:     {}", digest.dirty);
            match &digest.agent {
                Some(a) => {
                    println!(
                        "agent:     {} [{}]  attention={}",
                        a.kind,
                        a.status,
                        a.attention.as_str()
                    );
                    if let Some(h) = &a.headline {
                        println!("headline:  {h}");
                    }
                }
                None => println!("agent:     (none)"),
            }
            if let Some(p) = &pending_prompt {
                println!("pending:   {p}");
            }
            println!("--- transcript (last {}) ---", transcript.len());
            for t in &transcript {
                println!("[{}] {}", t.role, t.text);
            }
        }
    }
    Ok(())
}

/// Fetch a single lane's full state from the daemon (`lane.get`), shared by the read/send/approve
/// verbs.
async fn lane_get(client: &DaemonClient, lane: LaneId) -> Result<Lane> {
    client
        .call_typed("lane.get", Some(json!({ "lane_id": lane })))
        .await
}

/// The configured default agent kind, mirroring the MCP server: ask the daemon to detect agents
/// and pick the one flagged default, falling back to `claude-code` if detection fails.
async fn default_agent(client: &DaemonClient) -> String {
    match client
        .call_typed::<Vec<AgentChoice>>("agent.detect", None)
        .await
    {
        Ok(choices) => choices
            .into_iter()
            .find(|c| c.default)
            .map(|c| c.name)
            .unwrap_or_else(|| "claude-code".into()),
        Err(_) => "claude-code".into(),
    }
}

/// Resolve the spawn task prompt. `--task <text>` is used verbatim; `--task -` always reads the
/// prompt from stdin; omitting `--task` reads stdin when it is piped, or leaves the task unset on
/// an interactive terminal.
fn read_task(task: Option<String>) -> Result<Option<String>> {
    use std::io::{IsTerminal, Read};
    let from_stdin = match task.as_deref() {
        Some("-") => true,
        None => !std::io::stdin().is_terminal(),
        Some(_) => false,
    };
    if !from_stdin {
        return Ok(task);
    }
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| anyhow!("failed to read task from stdin: {e}"))?;
    let trimmed = buf.trim().to_string();
    Ok((!trimmed.is_empty()).then_some(trimmed))
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

const POWERSHELL_CD_WRAPPER: &str = r#"# repomon shell integration: cd into a lane's worktree on exit.
# Add to $PROFILE: repomon shell-init powershell | Out-String | Invoke-Expression
function repomon {
    $bin = (Get-Command -Name repomon -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1).Source
    if (-not $bin) { Write-Error 'repomon: binary not found on PATH'; return }
    $tmp = [System.IO.Path]::GetTempFileName()
    $env:REPOMON_CD_FILE = $tmp
    try { & $bin @args }
    finally { Remove-Item Env:\REPOMON_CD_FILE -ErrorAction SilentlyContinue }
    $dir = Get-Content -LiteralPath $tmp -TotalCount 1 -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
    if ($dir -and (Test-Path -LiteralPath $dir -PathType Container)) {
        Set-Location -LiteralPath $dir
    }
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
        clap_complete::Shell::PowerShell => POWERSHELL_CD_WRAPPER,
        other => {
            return Err(anyhow!(
                "shell-init: unsupported shell '{other}'; use zsh, bash, fish, or powershell"
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
    fn shell_init_powershell_defines_wrapper() {
        let out = super::shell_init(clap_complete::Shell::PowerShell).unwrap();
        assert!(out.contains("function repomon"));
        assert!(out.contains("$env:REPOMON_CD_FILE"));
        assert!(out.contains("Set-Location"));
        // The wrapper must invoke the real binary, not recurse into the function.
        assert!(out.contains("-CommandType Application"));
    }

    #[test]
    fn shell_init_rejects_unsupported_shell() {
        assert!(super::shell_init(clap_complete::Shell::Elvish).is_err());
    }

    #[test]
    fn tailscale_candidates_path_then_platform_fallback() {
        let [first, fallback] = super::tailscale_candidates();
        assert_eq!(first, "tailscale");
        if cfg!(windows) {
            assert_eq!(fallback, r"C:\Program Files\Tailscale\tailscale.exe");
        } else {
            assert_eq!(
                fallback,
                "/Applications/Tailscale.app/Contents/MacOS/Tailscale"
            );
        }
    }

    #[test]
    fn lane_spawn_binds_args() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from([
            "repomon",
            "lane",
            "spawn",
            "--lane",
            "6465124",
            "--agent",
            "codex",
            "--task",
            "do the thing",
        ])
        .expect("lane spawn should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Spawn {
                        lane, agent, task, ..
                    },
            }) => {
                assert_eq!(lane, 6465124);
                assert_eq!(agent.as_deref(), Some("codex"));
                assert_eq!(task.as_deref(), Some("do the thing"));
            }
            _ => panic!("expected `lane spawn`"),
        }
    }

    #[test]
    fn lane_spawn_agent_optional() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from(["repomon", "lane", "spawn", "--lane", "42"])
            .expect("lane spawn without --agent/--task should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Spawn {
                        lane,
                        agent,
                        task,
                        mode,
                        model,
                        effort,
                        force,
                    },
            }) => {
                assert_eq!(lane, 42);
                assert!(agent.is_none());
                assert!(task.is_none());
                // Mode defaults to Default; the other launch options are unset; force is off.
                assert_eq!(mode, super::SpawnMode::Default);
                assert!(model.is_none());
                assert!(effort.is_none());
                assert!(!force);
            }
            _ => panic!("expected `lane spawn`"),
        }
    }

    #[test]
    fn lane_spawn_force_binds() {
        use clap::Parser;
        let cli =
            crate::Cli::try_parse_from(["repomon", "lane", "spawn", "--lane", "42", "--force"])
                .expect("lane spawn --force should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd: super::LaneCmd::Spawn { force, .. },
            }) => assert!(force),
            _ => panic!("expected `lane spawn`"),
        }
    }

    #[test]
    fn lane_spawn_launch_options_bind() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from([
            "repomon", "lane", "spawn", "--lane", "42", "--mode", "plan", "--model", "opus",
            "--effort", "high",
        ])
        .expect("lane spawn with launch options should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Spawn {
                        mode,
                        model,
                        effort,
                        ..
                    },
            }) => {
                assert_eq!(mode, super::SpawnMode::Plan);
                assert_eq!(model.as_deref(), Some("opus"));
                assert_eq!(effort.as_deref(), Some("high"));
            }
            _ => panic!("expected `lane spawn`"),
        }
    }

    #[test]
    fn lane_spawn_rejects_bogus_mode() {
        use clap::Parser;
        // The ValueEnum constrains --mode to default|auto|plan, rejected at parse time.
        assert!(
            crate::Cli::try_parse_from([
                "repomon", "lane", "spawn", "--lane", "1", "--mode", "bogus"
            ])
            .is_err(),
            "`--mode bogus` must be rejected by the ValueEnum"
        );
    }

    #[test]
    fn lane_spawn_requires_lane() {
        use clap::Parser;
        assert!(
            crate::Cli::try_parse_from(["repomon", "lane", "spawn", "--task", "hi"]).is_err(),
            "`lane spawn` must require --lane"
        );
    }

    #[test]
    fn lane_send_binds_args() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from([
            "repomon",
            "lane",
            "send",
            "--lane",
            "42",
            "--text",
            "continue",
            "--no-submit",
            "--window",
            "lane-42-2",
        ])
        .expect("lane send should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Send {
                        lane,
                        text,
                        no_submit,
                        window,
                    },
            }) => {
                assert_eq!(lane, 42);
                assert_eq!(text.as_deref(), Some("continue"));
                assert!(no_submit);
                assert_eq!(window.as_deref(), Some("lane-42-2"));
            }
            _ => panic!("expected `lane send`"),
        }
    }

    #[test]
    fn lane_send_requires_lane() {
        use clap::Parser;
        assert!(
            crate::Cli::try_parse_from(["repomon", "lane", "send", "--text", "hi"]).is_err(),
            "`lane send` must require --lane"
        );
    }

    #[test]
    fn lane_approve_binds_args() {
        use clap::Parser;
        // Defaults: no choice, no window.
        let cli = crate::Cli::try_parse_from(["repomon", "lane", "approve", "--lane", "7"])
            .expect("lane approve should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Approve {
                        lane,
                        choice,
                        window,
                    },
            }) => {
                assert_eq!(lane, 7);
                assert!(choice.is_none());
                assert!(window.is_none());
            }
            _ => panic!("expected `lane approve`"),
        }
        // An explicit choice binds.
        let cli = crate::Cli::try_parse_from([
            "repomon", "lane", "approve", "--lane", "7", "--choice", "no",
        ])
        .expect("lane approve --choice should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd: super::LaneCmd::Approve { choice, .. },
            }) => assert_eq!(choice.as_deref(), Some("no")),
            _ => panic!("expected `lane approve`"),
        }
    }

    #[test]
    fn lane_interrupt_binds_args() {
        use clap::Parser;
        let cli =
            crate::Cli::try_parse_from(["repomon", "lane", "interrupt", "--lane", "9", "--hard"])
                .expect("lane interrupt should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd: super::LaneCmd::Interrupt { lane, hard, window },
            }) => {
                assert_eq!(lane, 9);
                assert!(hard);
                assert!(window.is_none());
            }
            _ => panic!("expected `lane interrupt`"),
        }
    }

    #[test]
    fn lane_read_binds_args() {
        use clap::Parser;
        // Default transcript limit is 12.
        let cli = crate::Cli::try_parse_from(["repomon", "lane", "read", "--lane", "3"])
            .expect("lane read should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Read {
                        lane,
                        transcript_limit,
                    },
            }) => {
                assert_eq!(lane, 3);
                assert_eq!(transcript_limit, 12);
            }
            _ => panic!("expected `lane read`"),
        }
        // An explicit limit overrides.
        let cli = crate::Cli::try_parse_from([
            "repomon",
            "lane",
            "read",
            "--lane",
            "3",
            "--transcript-limit",
            "40",
        ])
        .expect("lane read --transcript-limit should parse");
        match cli.command {
            Some(super::Command::Lane {
                cmd:
                    super::LaneCmd::Read {
                        transcript_limit, ..
                    },
            }) => assert_eq!(transcript_limit, 40),
            _ => panic!("expected `lane read`"),
        }
    }

    #[test]
    fn approve_key_mapping_is_reused_from_mcp() {
        // The CLI `lane approve` verb reuses the MCP server's approve_key mapping verbatim.
        use repomon_mcp::server::approve_key;
        assert_eq!(approve_key(None).unwrap().0, "Enter");
        assert_eq!(
            approve_key(Some(&serde_json::json!("no"))).unwrap().0,
            "Escape"
        );
        assert_eq!(approve_key(Some(&serde_json::json!("2"))).unwrap().0, "2");
        assert!(approve_key(Some(&serde_json::json!("maybe"))).is_err());
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

    #[test]
    fn msg_send_and_list_bind_the_public_cli() {
        use clap::Parser;
        let cli = crate::Cli::try_parse_from([
            "repomon",
            "msg",
            "send",
            "lane-7/2",
            "please review",
            "--reply-to",
            "mail-1",
        ])
        .expect("msg send should parse");
        match cli.command {
            Some(super::Command::Msg {
                cmd:
                    super::MsgCmd::Send {
                        address,
                        body,
                        reply_to,
                    },
            }) => {
                assert_eq!(address, "lane-7/2");
                assert_eq!(body, "please review");
                assert_eq!(reply_to.as_deref(), Some("mail-1"));
            }
            _ => panic!("expected `msg send`"),
        }
        let cli = crate::Cli::try_parse_from([
            "repomon", "msg", "list", "--unread", "--lane", "7", "--limit", "20",
        ])
        .expect("msg list should parse");
        match cli.command {
            Some(super::Command::Msg {
                cmd:
                    super::MsgCmd::List {
                        unread,
                        lane,
                        limit,
                    },
            }) => {
                assert!(unread);
                assert_eq!(lane, Some(7));
                assert_eq!(limit, 20);
            }
            _ => panic!("expected `msg list`"),
        }
    }
}
