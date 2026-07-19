//! Host process assembly (Windows only): parse the spawn contract, start the ConPTY child,
//! register, and serve until the child dies or a `kill` arrives.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use clap::Parser as _;
use tokio::sync::broadcast;

use crate::cli::HostArgs;
use crate::dacl::PipeSecurity;
use crate::dispatch::{Dispatcher, HostMeta};
use crate::screen::Screen;
use crate::server::{self, ServerCtx, epoch_now};
use crate::{pty, registry};

pub fn windows_main() -> ExitCode {
    let args = HostArgs::parse();
    match run(args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("repomon-agent-host: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: HostArgs) -> anyhow::Result<ExitCode> {
    let owner = args
        .owner
        .clone()
        .unwrap_or_else(registry::generate_owner_token);
    let (program, program_args) = args
        .command
        .split_first()
        .expect("clap enforces a non-empty command");

    let spawned = pty::spawn_child(
        program,
        program_args,
        &args.cwd,
        &args.env,
        args.cols,
        args.rows,
    )?;
    let started_at = epoch_now();

    let meta = HostMeta {
        session: args.session.clone(),
        window: args.window.clone(),
        cwd: args.cwd.display().to_string(),
        program: program.clone(),
        args: program_args.to_vec(),
        agent_pid: spawned.child_pid,
        owner: owner.clone(),
        started_at,
    };

    let data_dir = registry::data_dir();
    let registry_path = registry::registry_path(&data_dir, &args.session, &args.window);
    let pipe = registry::pipe_name(&args.session, &args.window);

    let entry = registry::RegistryEntry {
        v: 1,
        session: args.session.clone(),
        window: args.window.clone(),
        pipe: pipe.clone(),
        host_pid: std::process::id(),
        agent_pid: spawned.child_pid,
        program: program.clone(),
        args: program_args.to_vec(),
        cwd: args.cwd.display().to_string(),
        owner,
        started_at,
    };

    let (bytes_tx, _) = broadcast::channel::<Vec<u8>>(1024);
    let ctx = Arc::new(ServerCtx {
        dispatcher: Mutex::new(Dispatcher::new(
            meta,
            Screen::new(args.cols, args.rows),
            Box::new(spawned.controller),
        )),
        bytes_tx: bytes_tx.clone(),
        registry_path: registry_path.clone(),
    });

    spawn_reader_thread(spawned.reader, ctx.clone());
    spawn_waiter_thread(spawned.child, registry_path.clone());

    let security = PipeSecurity::current_user_only()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        // Listen first, then register: a registry entry implies a connectable pipe
        // (PROTOCOL.md §8).
        let first_instance = server::create_instance(&pipe, &security, true)?;
        registry::write_atomic(&registry_path, &entry)?;
        server::serve(pipe, security, first_instance, ctx).await
    })?;
    Ok(ExitCode::SUCCESS)
}

/// Drain ConPTY output: feed the vt100 screen (bumping `last_activity`) and fan the raw
/// chunk out to byte subscribers. EOF means the child is gone — the waiter thread owns the
/// shutdown.
fn spawn_reader_thread(mut reader: Box<dyn std::io::Read + Send>, ctx: Arc<ServerCtx>) {
    std::thread::spawn(move || {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    let chunk = &buf[..n];
                    {
                        let mut dispatcher = ctx.dispatcher.lock().expect("dispatcher lock");
                        dispatcher.process_output(chunk, epoch_now());
                        // Send while holding the lock: subscribers snapshot-then-subscribe
                        // under the same lock, so replay/live can never gap or overlap.
                        let _ = ctx.bytes_tx.send(chunk.to_vec());
                    }
                }
            }
        }
    });
}

/// Wait for the agent child; on exit, linger briefly (lets an in-flight `kill` response
/// flush), remove the registry entry, and exit — the window disappears, tmux-style.
fn spawn_waiter_thread(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    registry_path: std::path::PathBuf,
) {
    std::thread::spawn(move || {
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = registry::remove(&registry_path);
        std::process::exit(0);
    });
}
