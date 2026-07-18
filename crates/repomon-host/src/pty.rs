//! ConPTY child management via `portable-pty` (Windows only).
//!
//! `CommandBuilder` gets the structured `program + args + cwd + env` straight from the CLI —
//! no shell strings, no `cmd /c` quoting — and it resolves npm `.cmd` shims (`claude`) the
//! way `CreateProcess` alone would not.

use std::path::Path;

use anyhow::Context as _;
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::dispatch::PtyIo;

/// Everything `spawn_child` hands back: the dispatcher-side controller, the raw output
/// reader (drained by a dedicated thread), and the child (waited by another thread).
pub struct SpawnedChild {
    pub controller: PtyController,
    pub reader: Box<dyn std::io::Read + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub child_pid: u32,
}

/// The dispatcher's handle on the ConPTY: input writes, resizes, kills.
pub struct PtyController {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl PtyIo for PtyController {
    fn write(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.writer
            .write_all(bytes)
            .context("write to ConPTY input")?;
        self.writer.flush().context("flush ConPTY input")?;
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resize ConPTY")
    }

    fn kill(&mut self) -> anyhow::Result<()> {
        self.killer.kill().context("kill agent child")
    }
}

/// Open a `cols × rows` ConPTY and spawn the agent child on it.
pub fn spawn_child(
    program: &str,
    args: &[String],
    cwd: &Path,
    env: &[(String, String)],
    cols: u16,
    rows: u16,
) -> anyhow::Result<SpawnedChild> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("open ConPTY")?;

    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.cwd(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawn {program:?}"))?;
    // The slave side belongs to the child now; dropping our handle lets child-exit surface
    // as EOF on the reader.
    drop(pair.slave);

    let child_pid = child.process_id().unwrap_or(0);
    let killer = child.clone_killer();
    let reader = pair
        .master
        .try_clone_reader()
        .context("clone ConPTY reader")?;
    let writer = pair.master.take_writer().context("take ConPTY writer")?;

    Ok(SpawnedChild {
        controller: PtyController {
            master: pair.master,
            writer,
            killer,
        },
        reader,
        child,
        child_pid,
    })
}
