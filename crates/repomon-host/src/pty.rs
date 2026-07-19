//! ConPTY child management via `portable-pty` (Windows only).
//!
//! `CommandBuilder` gets the structured `program + args + cwd + env` straight from the CLI —
//! no shell strings, no `cmd /c` quoting — and it resolves npm `.cmd` shims (`claude`) the
//! way `CreateProcess` alone would not.

use std::path::Path;

use anyhow::Context as _;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::dispatch::PtyIo;

/// Everything `spawn_child` hands back: the dispatcher-side controller, the raw output
/// reader (drained by a dedicated thread), and the child (waited by another thread).
pub struct SpawnedChild {
    pub controller: PtyController,
    pub reader: Box<dyn std::io::Read + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub child_pid: u32,
}

/// An owned, inheritable-safe duplicate of the ConPTY child's process handle.
///
/// We terminate the child ourselves rather than via `portable_pty`'s `ChildKiller`, whose
/// Windows `WinChildKiller::kill` has inverted success/error semantics in 0.9: `TerminateProcess`
/// returns nonzero on success, but the killer returns `Err(last_os_error())` on that success (a
/// stale `ERROR_IO_INCOMPLETE`/996) and `Ok(())` on the actual failure. Owning a handle and
/// calling `TerminateProcess` directly gives us correct semantics.
struct ChildHandle(windows_sys::Win32::Foundation::HANDLE);

// The handle is an OS process handle we own for the controller's lifetime; sending it across
// threads (the controller lives behind the dispatcher mutex) is sound.
unsafe impl Send for ChildHandle {}

impl Drop for ChildHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
        }
    }
}

/// The dispatcher's handle on the ConPTY: input writes, resizes, kills.
pub struct PtyController {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn std::io::Write + Send>,
    child: ChildHandle,
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
        use windows_sys::Win32::Foundation::STILL_ACTIVE;
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, TerminateProcess};
        // `TerminateProcess` returns nonzero on success. A zero return with the child already
        // gone (`GetExitCodeProcess` != STILL_ACTIVE) is a benign race — the window is dying
        // either way — so we treat it as success.
        let ok = unsafe { TerminateProcess(self.child.0, 1) };
        if ok != 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        let mut code: u32 = 0;
        let exited = unsafe { GetExitCodeProcess(self.child.0, &mut code) } != 0
            && code != STILL_ACTIVE as u32;
        if exited {
            return Ok(());
        }
        Err(anyhow::anyhow!("kill agent child: {err}"))
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
    // Own an independent duplicate of the child's process handle for `kill` (see `ChildHandle`);
    // the original handle stays with `child`, which the waiter thread owns.
    let child_handle = duplicate_child_handle(child.as_ref())?;
    let reader = pair
        .master
        .try_clone_reader()
        .context("clone ConPTY reader")?;
    let writer = pair.master.take_writer().context("take ConPTY writer")?;

    Ok(SpawnedChild {
        controller: PtyController {
            master: pair.master,
            writer,
            child: child_handle,
        },
        reader,
        child,
        child_pid,
    })
}

/// Duplicate the ConPTY child's process handle into one this process owns independently, so the
/// controller can `TerminateProcess` it without racing the waiter thread that owns the original.
fn duplicate_child_handle(child: &(dyn Child + Send + Sync)) -> anyhow::Result<ChildHandle> {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let raw = child
        .as_raw_handle()
        .context("ConPTY child has no process handle")? as HANDLE;
    let mut dup: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        let me = GetCurrentProcess();
        DuplicateHandle(me, raw, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS)
    };
    if ok == 0 {
        return Err(anyhow::anyhow!(
            "duplicate ConPTY child handle: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(ChildHandle(dup))
}
