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
pub enum DaemonCmd {
    /// Write the launchd plist and load it.
    Install,
    /// Unload and remove the launchd plist.
    Uninstall,
    /// Install (if needed) and (re)start the daemon.
    Start,
    /// Stop the daemon.
    Stop,
    /// Show daemon status.
    Status,
    /// Print the daemon log (tail).
    Logs,
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
        Command::Daemon { cmd } => handle_daemon(cmd, config)?,
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

fn handle_daemon(cmd: DaemonCmd, config: &Config) -> Result<()> {
    let socket = config::socket_path(config);
    match cmd {
        DaemonCmd::Install => {
            service::install(&service::repomond_path(), &socket)?;
            println!("installed and loaded {}", service::plist_path().display());
        }
        DaemonCmd::Uninstall => {
            service::uninstall()?;
            println!("uninstalled");
        }
        DaemonCmd::Start => {
            // Install is idempotent; ensure the plist exists, then (re)start.
            service::install(&service::repomond_path(), &socket)?;
            service::start()?;
            println!("daemon started (socket: {})", socket.display());
        }
        DaemonCmd::Stop => {
            service::stop()?;
            println!("daemon stopped");
        }
        DaemonCmd::Status => {
            print!("{}", service::status()?);
        }
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
    }
    Ok(())
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
