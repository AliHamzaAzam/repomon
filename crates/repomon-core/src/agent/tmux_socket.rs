//! Socket discovery and recovery. Never interpret an unreachable living server as an empty fleet.
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

#[derive(Debug, Default)]
pub(super) struct SocketState {
    path: Option<PathBuf>,
    managed_override: Option<PathBuf>,
    legacy_override: Option<Vec<PathBuf>>,
    #[cfg(any(test, feature = "test-support"))]
    _test_root: Option<tempfile::TempDir>,
    server: Option<(u32, String)>,
    inode: Option<(u64, u64)>,
    touched: Option<Instant>,
}

pub fn managed_socket(session: &str) -> PathBuf {
    crate::config::runtime_dir().join("tmux").join(session)
}

#[cfg(unix)]
fn legacy_socket(session: &str) -> PathBuf {
    let root = std::env::var_os("TMUX_TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    root.join(format!("tmux-{}", unsafe { libc::getuid() }))
        .join(session)
}

pub(super) fn inode(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let m = std::fs::symlink_metadata(path).ok()?;
        m.file_type().is_socket().then(|| (m.dev(), m.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Refresh socket timestamps without opening a stream or following a substituted symlink.
pub fn touch_socket(path: &Path) -> std::io::Result<()> {
    if inode(path).is_some() {
        let now = filetime::FileTime::now();
        filetime::set_symlink_file_times(path, now, now)?;
    }
    Ok(())
}

fn query(program: &Path, path: &Path) -> Option<u32> {
    let out = Command::new(program)
        .arg("-S")
        .arg(path)
        .args(["display-message", "-p", "#{pid}"])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .flatten()
}

#[cfg(unix)]
fn prepare_parent(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let parent = path
        .parent()
        .ok_or_else(|| Error::Agent("tmux socket has no parent".into()))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(Error::Agent(format!(
            "tmux socket directory must be owned by this user with mode 0700: {}",
            parent.display()
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn prepare_parent(_path: &Path) -> Result<()> {
    Ok(())
}

/// Discover unlinked sockets through open descriptors, not process-name guesses. lsof retains
/// Unix socket names on macOS; Linux exposes the same association through /proc/net/unix.
#[cfg(target_os = "linux")]
fn discover(path: &Path) -> Result<Vec<u32>> {
    let output = Command::new("ps")
        .args(["-axo", "pid=,uid=,comm="])
        .output()?;
    if !output.status.success() {
        return Err(Error::Agent("cannot inspect tmux server processes".into()));
    }
    let uid = unsafe { libc::getuid() }.to_string();
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(pid) = fields.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        if fields.next() != Some(uid.as_str()) {
            continue;
        }
        let command = fields.collect::<Vec<_>>().join(" ");
        if !command.contains("tmux") {
            continue;
        }
        if process_has_socket(pid, path)? {
            found.push(pid);
        }
    }
    Ok(found)
}

#[cfg(target_os = "linux")]
fn process_has_socket(pid: u32, path: &Path) -> Result<bool> {
    let table = std::fs::read_to_string("/proc/net/unix")?;
    let inodes: Vec<_> = table
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            (fields.len() >= 8 && Path::new(fields[7]) == path)
                .then(|| format!("socket:[{}]", fields[6]))
        })
        .collect();
    let Ok(entries) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
        return Ok(false);
    };
    Ok(entries.flatten().any(|entry| {
        std::fs::read_link(entry.path()).is_ok_and(|p| inodes.iter().any(|i| p == Path::new(i)))
    }))
}

#[cfg(all(unix, not(target_os = "linux")))]
fn discover(path: &Path) -> Result<Vec<u32>> {
    let output = Command::new("/usr/sbin/lsof")
        .args([
            "-a",
            "-u",
            &unsafe { libc::getuid() }.to_string(),
            "-U",
            "-Fpcn",
        ])
        .output()?;
    if !output.status.success() && !output.stderr.is_empty() {
        return Err(Error::Agent(format!(
            "cannot discover tmux sockets: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(socket_owners(
        &String::from_utf8_lossy(&output.stdout),
        path,
    ))
}

#[cfg(any(all(test, unix), all(unix, not(target_os = "linux"))))]
fn socket_owners(output: &str, path: &Path) -> Vec<u32> {
    let normalize = |p: &str| {
        p.strip_prefix("/private/tmp/")
            .map(|rest| format!("/tmp/{rest}"))
            .unwrap_or_else(|| p.to_string())
    };
    let expected = normalize(&path.to_string_lossy());
    let (mut pid, mut tmux) = (None, false);
    let mut found = Vec::new();
    for line in output.lines() {
        if let Some(value) = line.strip_prefix('p') {
            pid = value.parse().ok();
            tmux = false;
        }
        if let Some(command) = line.strip_prefix('c') {
            tmux = command == "tmux" || command.starts_with("tmux:");
        }
        if let Some(name) = line.strip_prefix('n') {
            if tmux && normalize(name.strip_suffix(" (deleted)").unwrap_or(name)) == expected {
                if let Some(pid) = pid {
                    found.push(pid);
                }
            }
        }
    }
    found.sort_unstable();
    found.dedup();
    found
}

#[cfg(not(unix))]
fn discover(_path: &Path) -> Result<Vec<u32>> {
    Ok(Vec::new())
}

trait SocketOps {
    fn query(&self, path: &Path) -> Option<u32>;
    fn has_session(&self, path: &Path, session: &str) -> bool;
    fn discover(&self, path: &Path) -> Result<Vec<u32>>;
    fn fingerprint(&self, pid: u32) -> Option<String>;
    fn running(&self, pid: u32) -> bool;
    fn inode(&self, path: &Path) -> Option<(u64, u64)>;
    fn prepare(&self, path: &Path) -> Result<()>;
    fn signal(&self, pid: u32) -> Result<()>;
    fn pause(&self);
    fn touch(&self, path: &Path) -> Result<()>;
}

struct SystemOps<'a>(&'a Path);
impl SocketOps for SystemOps<'_> {
    fn query(&self, path: &Path) -> Option<u32> {
        query(self.0, path)
    }
    fn has_session(&self, path: &Path, session: &str) -> bool {
        Command::new(self.0)
            .arg("-N")
            .arg("-S")
            .arg(path)
            .args(["has-session", "-t", &format!("={session}")])
            .output()
            .is_ok_and(|out| out.status.success())
    }
    fn discover(&self, path: &Path) -> Result<Vec<u32>> {
        discover(path)
    }
    fn fingerprint(&self, pid: u32) -> Option<String> {
        super::tmux::process_fingerprint(pid)
    }
    fn running(&self, pid: u32) -> bool {
        #[cfg(unix)]
        {
            (unsafe { libc::kill(pid as libc::pid_t, 0) }) == 0
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
        }
        #[cfg(not(unix))]
        {
            let _ = pid;
            false
        }
    }
    fn inode(&self, path: &Path) -> Option<(u64, u64)> {
        inode(path)
    }
    fn prepare(&self, path: &Path) -> Result<()> {
        prepare_parent(path)
    }
    fn signal(&self, pid: u32) -> Result<()> {
        #[cfg(unix)]
        if unsafe { libc::kill(pid as libc::pid_t, libc::SIGUSR1) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        #[cfg(not(unix))]
        let _ = pid;
        Ok(())
    }
    fn pause(&self) {
        std::thread::sleep(Duration::from_millis(50));
    }
    fn touch(&self, path: &Path) -> Result<()> {
        Ok(touch_socket(path)?)
    }
}

fn alive(ops: &impl SocketOps, server: &(u32, String)) -> Result<bool> {
    match ops.fingerprint(server.0) {
        Some(start) => Ok(start == server.1),
        None if ops.running(server.0) => Err(Error::Agent(format!(
            "cannot verify living tmux process {}; refusing recovery or fleet replacement",
            server.0
        ))),
        None => Ok(false),
    }
}

fn recover(ops: &impl SocketOps, path: &Path, server: &(u32, String)) -> Result<()> {
    if !alive(ops, server)? {
        return Err(Error::Agent("tmux process changed before recovery".into()));
    }
    ops.prepare(path)?;
    ops.signal(server.0)?;
    for _ in 0..20 {
        if ops.query(path) == Some(server.0) {
            tracing::warn!(pid = server.0, path = %path.display(), "recovered tmux socket with SIGUSR1; adopting existing fleet");
            return Ok(());
        }
        ops.pause();
    }
    Err(Error::Agent(format!(
        "tmux server {} is alive but unreachable; refusing to replace its fleet",
        server.0
    )))
}

fn missing_server(out: &Output) -> bool {
    let error = String::from_utf8_lossy(&out.stderr);
    error.contains("no server running") || error.contains("error connecting")
}

impl SocketState {
    fn managed(&self, session: &str) -> PathBuf {
        self.managed_override
            .clone()
            .unwrap_or_else(|| managed_socket(session))
    }

    pub(super) fn legacy(&self, session: &str) -> Vec<PathBuf> {
        if let Some(paths) = &self.legacy_override {
            return paths.clone();
        }
        #[cfg(unix)]
        {
            vec![legacy_socket(session)]
        }
        #[cfg(not(unix))]
        {
            let _ = session;
            Vec::new()
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn isolated() -> Self {
        #[cfg(unix)]
        let root = tempfile::Builder::new()
            .prefix("repomon-test-")
            .tempdir_in("/tmp")
            .expect("temporary tmux directory");
        #[cfg(not(unix))]
        let root = tempfile::tempdir().expect("temporary tmux directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private temporary tmux directory");
        }
        Self {
            managed_override: Some(root.path().join("managed.sock")),
            legacy_override: Some(vec![root.path().join("legacy.sock")]),
            _test_root: Some(root),
            path: None,
            server: None,
            inode: None,
            touched: None,
        }
    }

    pub(super) fn path(&self, session: &str) -> PathBuf {
        self.path.clone().unwrap_or_else(|| self.managed(session))
    }

    pub(super) fn ensure(
        &mut self,
        program: &Path,
        session: &str,
        creating: bool,
    ) -> Result<PathBuf> {
        self.ensure_with(&SystemOps(program), session, creating)
    }

    fn ensure_with(
        &mut self,
        ops: &impl SocketOps,
        session: &str,
        creating: bool,
    ) -> Result<PathBuf> {
        let managed = self.managed(session);
        if self.path.is_none() {
            let mut candidates = vec![managed.clone()];
            candidates.extend(self.legacy(session));
            let mut servers = Vec::new();
            for path in candidates {
                let mut pids = ops.discover(&path)?;
                if let Some(pid) = ops.query(&path) {
                    pids.push(pid);
                }
                pids.sort_unstable();
                pids.dedup();
                for pid in pids {
                    match ops.fingerprint(pid) {
                        Some(start) => servers.push((path.clone(), (pid, start))),
                        None if ops.running(pid) => {
                            return Err(Error::Agent(format!(
                                "cannot verify living tmux process {pid}; refusing fleet replacement"
                            )));
                        }
                        None => {}
                    }
                }
            }
            if servers.len() > 1 {
                let reachable: Vec<_> = servers
                    .iter()
                    .enumerate()
                    .filter(|(_, (path, (pid, _)))| {
                        ops.query(path) == Some(*pid) && ops.has_session(path, session)
                    })
                    .map(|(index, _)| index)
                    .collect();
                if reachable.len() != 1 {
                    let candidates: Vec<_> = servers
                        .iter()
                        .map(|(path, (pid, _))| format!("{pid}@{}", path.display()))
                        .collect();
                    let message = format!(
                        "multiple tmux servers own session {session}; no unique reachable session owner; refusing recovery and fleet replacement; candidates: {}",
                        candidates.join(", ")
                    );
                    tracing::error!("{message}");
                    return Err(Error::Agent(message));
                }
                let selected = servers.remove(reachable[0]);
                let preserved: Vec<_> = servers.iter().map(|(_, (pid, _))| *pid).collect();
                tracing::warn!(session, pid = selected.1.0, path = %selected.0.display(), ?preserved,
                    "adopting unique reachable tmux session owner; leaving other surviving servers untouched; replacement server creation remains guarded");
                servers = vec![selected];
            }
            if let Some((path, server)) = servers.pop() {
                if ops.query(&path) != Some(server.0) {
                    recover(ops, &path, &server)?;
                }
                if path != managed {
                    tracing::warn!(pid = server.0, path = %path.display(), "adopting legacy tmux fleet; runtime socket takes effect after this server exits");
                }
                self.inode = ops.inode(&path);
                self.path = Some(path);
                self.server = Some(server);
            } else {
                self.path = Some(managed.clone());
            }
        }
        let mut path = self.path(session);
        if let Some(server) = &self.server {
            if ops.inode(&path) != self.inode || self.inode.is_none() {
                if alive(ops, server)? {
                    if let Some(other) = ops.query(&path) {
                        if other != server.0 {
                            return Err(Error::Agent("tmux socket was replaced while the prior server is alive; refusing fleet replacement".into()));
                        }
                    }
                    recover(ops, &path, server)?;
                    self.inode = ops.inode(&path);
                } else {
                    self.server = None;
                    self.inode = None;
                    self.path = Some(managed);
                    path = self.path(session);
                }
            }
        }
        if creating {
            if let Some(server) = &self.server {
                if !alive(ops, server)? {
                    self.server = None;
                    self.inode = None;
                }
            }
        }
        if creating && self.server.is_none() {
            // A fresh handle or another daemon may have discovered a surviving fleet since our
            // last empty probe. Recheck before any command capable of starting a server.
            self.path = None;
            path = self.ensure_with(ops, session, false)?;
            ops.prepare(&path)?;
        }
        if self
            .touched
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(86400))
        {
            ops.touch(&path)?;
            self.touched = Some(Instant::now());
        }
        Ok(path)
    }

    pub(super) fn record(&mut self, program: &Path, path: &Path) {
        self.path = None;
        self.server = None;
        self.inode = None;
        if let Some(pid) = query(program, path) {
            if let Some(start) = super::tmux::process_fingerprint(pid) {
                self.path = Some(path.to_path_buf());
                self.server = Some((pid, start));
                self.inode = inode(path);
            }
        }
    }

    pub(super) fn execute(
        &mut self,
        program: &Path,
        session: &str,
        args: &[&str],
        locale: Option<(&str, &str)>,
    ) -> Result<Output> {
        let creating = matches!(args.first(), Some(&"new-session") | Some(&"new-window"));
        let path = self.ensure(program, session, creating)?;
        let mut command = Command::new(program);
        // Close the check-to-command race: tmux itself must refuse to start a second server if
        // the known server's socket is removed between ensure() and command execution.
        if creating && self.server.is_some() {
            command.arg("-N");
        }
        command.arg("-S").arg(&path).args(args);
        if let Some((key, value)) = locale {
            command.env(key, value);
        }
        let out = command.output()?;
        if creating && out.status.success() {
            self.record(program, &path);
        }
        // A failed query must not be turned into an empty fleet while its server still lives.
        if !out.status.success() && missing_server(&out) {
            if let Some(server) = &self.server {
                if alive(&SystemOps(program), server)? {
                    recover(&SystemOps(program), &path, server)?;
                    self.inode = inode(&path);
                    let retried = command.output()?;
                    if !retried.status.success() && missing_server(&retried) {
                        return Err(Error::Agent(format!(
                            "tmux server {} remains alive but unreachable after recovery; refusing fleet replacement",
                            server.0
                        )));
                    }
                    return Ok(retried);
                }
            }
        }
        Ok(out)
    }
}

#[cfg(all(unix, any(test, feature = "test-support")))]
impl Drop for SocketState {
    fn drop(&mut self) {
        let Some(root) = self._test_root.as_ref() else {
            return;
        };
        for path in [
            root.path().join("managed.sock"),
            root.path().join("legacy.sock"),
        ] {
            let Ok(pids) = discover(&path) else {
                continue;
            };
            for pid in pids {
                let Some(start) = super::tmux::process_fingerprint(pid) else {
                    continue;
                };
                if discover(&path).is_ok_and(|owners| owners.contains(&pid))
                    && super::tmux::process_fingerprint(pid).as_ref() == Some(&start)
                {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGTERM);
                    }
                    for _ in 0..100 {
                        if super::tmux::process_fingerprint(pid).as_ref() != Some(&start) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    #[derive(Default)]
    struct Scripted {
        servers: RefCell<HashMap<PathBuf, u32>>,
        reachable: RefCell<HashMap<PathBuf, u32>>,
        starts: RefCell<HashMap<u32, String>>,
        signals: RefCell<Vec<u32>>,
        refuse_recovery: Cell<bool>,
        unreadable_start: Cell<bool>,
    }
    impl Scripted {
        fn add(&self, path: PathBuf, pid: u32, reachable: bool) {
            self.servers.borrow_mut().insert(path.clone(), pid);
            self.starts.borrow_mut().insert(pid, format!("start-{pid}"));
            if reachable {
                self.reachable.borrow_mut().insert(path, pid);
            }
        }
    }
    impl SocketOps for Scripted {
        fn query(&self, path: &Path) -> Option<u32> {
            self.reachable.borrow().get(path).copied()
        }
        fn has_session(&self, path: &Path, _session: &str) -> bool {
            self.query(path).is_some()
        }
        fn discover(&self, path: &Path) -> Result<Vec<u32>> {
            Ok(self
                .servers
                .borrow()
                .get(path)
                .copied()
                .into_iter()
                .collect())
        }
        fn fingerprint(&self, pid: u32) -> Option<String> {
            if self.unreadable_start.get() {
                None
            } else {
                self.starts.borrow().get(&pid).cloned()
            }
        }
        fn running(&self, pid: u32) -> bool {
            self.starts.borrow().contains_key(&pid)
        }
        fn inode(&self, path: &Path) -> Option<(u64, u64)> {
            self.query(path).map(|pid| (1, pid as u64))
        }
        fn prepare(&self, _path: &Path) -> Result<()> {
            Ok(())
        }
        fn touch(&self, _path: &Path) -> Result<()> {
            Ok(())
        }
        fn pause(&self) {}
        fn signal(&self, pid: u32) -> Result<()> {
            self.signals.borrow_mut().push(pid);
            if !self.refuse_recovery.get() {
                for (path, owner) in self.servers.borrow().iter() {
                    if *owner == pid {
                        self.reachable.borrow_mut().insert(path.clone(), pid);
                    }
                }
            }
            Ok(())
        }
    }

    #[test]
    fn adopts_live_legacy_and_recovers_unlinked_legacy_without_new_server() {
        for reachable in [true, false] {
            let ops = Scripted::default();
            let legacy = legacy_socket("i1-scripted");
            ops.add(legacy.clone(), 42, reachable);
            let mut state = SocketState::default();
            assert_eq!(
                state.ensure_with(&ops, "i1-scripted", true).unwrap(),
                legacy
            );
            assert_eq!(state.server.as_ref().map(|s| s.0), Some(42));
            assert_eq!(
                *ops.signals.borrow(),
                if reachable { vec![] } else { vec![42] }
            );
        }
    }

    #[test]
    fn missing_known_socket_recovers_or_blocks_every_create_attempt() {
        let ops = Scripted::default();
        let path = managed_socket("i1-scripted");
        ops.add(path.clone(), 42, true);
        let mut state = SocketState::default();
        state.ensure_with(&ops, "i1-scripted", false).unwrap();
        ops.reachable.borrow_mut().clear();
        ops.refuse_recovery.set(true);
        for _ in 0..2 {
            assert!(state.ensure_with(&ops, "i1-scripted", true).is_err());
        }
        assert_eq!(state.server.as_ref().map(|s| s.0), Some(42));
        ops.refuse_recovery.set(false);
        assert_eq!(state.ensure_with(&ops, "i1-scripted", true).unwrap(), path);
    }

    #[test]
    fn unreadable_start_time_never_turns_a_living_server_into_an_empty_fleet() {
        let ops = Scripted::default();
        ops.add(managed_socket("i1-scripted"), 42, true);
        let mut state = SocketState::default();
        state.ensure_with(&ops, "i1-scripted", false).unwrap();
        ops.reachable.borrow_mut().clear();
        ops.unreadable_start.set(true);
        assert!(state.ensure_with(&ops, "i1-scripted", true).is_err());
        assert!(
            SocketState::default()
                .ensure_with(&ops, "i1-scripted", true)
                .is_err()
        );
        assert!(ops.signals.borrow().is_empty());
    }

    #[test]
    fn second_server_and_replaced_socket_block_restore() {
        let ops = Scripted::default();
        let path = managed_socket("i1-scripted");
        ops.add(path.clone(), 42, true);
        let mut state = SocketState::default();
        state.ensure_with(&ops, "i1-scripted", false).unwrap();
        ops.reachable.borrow_mut().insert(path, 99);
        assert!(state.ensure_with(&ops, "i1-scripted", true).is_err());
        assert!(ops.signals.borrow().is_empty());
        ops.add(legacy_socket("i1-scripted"), 77, false);
        assert!(
            SocketState::default()
                .ensure_with(&ops, "i1-scripted", true)
                .is_err()
        );
    }

    #[test]
    fn unique_reachable_session_is_adopted_but_two_reachable_owners_are_ambiguous() {
        let ops = Scripted::default();
        let path = managed_socket("i1-scripted");
        let legacy = legacy_socket("i1-scripted");
        ops.add(path.clone(), 42, true);
        ops.add(legacy.clone(), 77, false);
        let mut state = SocketState::default();
        assert_eq!(state.ensure_with(&ops, "i1-scripted", true).unwrap(), path);
        assert_eq!(state.server.as_ref().map(|s| s.0), Some(42));
        assert!(ops.signals.borrow().is_empty());
        ops.reachable.borrow_mut().insert(legacy, 77);
        assert!(
            SocketState::default()
                .ensure_with(&ops, "i1-scripted", true)
                .is_err()
        );
        ops.reachable.borrow_mut().clear();
        assert!(
            SocketState::default()
                .ensure_with(&ops, "i1-scripted", true)
                .is_err()
        );
        assert!(ops.signals.borrow().is_empty());
    }

    fn start_test_server(path: &Path, session: &str, window: &str) -> (u32, String) {
        let program = super::super::tmux::tmux_program();
        let out = Command::new(&program)
            .arg("-S")
            .arg(path)
            .args(["new-session", "-d", "-s", session, "-n", window, "sleep 60"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let pid = query(&program, path).unwrap();
        (pid, super::super::tmux::process_fingerprint(pid).unwrap())
    }

    #[test]
    fn duplicate_legacy_servers_adopt_reachable_owner_and_cleanup_both_verified_pids() {
        use crate::agent::TmuxRuntime;
        if !TmuxRuntime::available() {
            return;
        }
        let runtime = TmuxRuntime::isolated("repomon");
        let path = runtime.test_legacy_socket_path();
        let root = path.parent().unwrap().to_path_buf();
        let old = start_test_server(&path, "repomon", "preserved");
        std::fs::remove_file(&path).unwrap();
        let active = start_test_server(&path, "repomon", "active");
        assert_ne!(old.0, active.0);
        assert_eq!(runtime.list_windows().unwrap(), ["active"]);
        assert_eq!(
            super::super::tmux::process_fingerprint(old.0),
            Some(old.1.clone())
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(runtime.list_windows().unwrap(), ["active"]);
        assert_eq!(
            query(&super::super::tmux::tmux_program(), &path),
            Some(active.0)
        );
        assert_eq!(
            super::super::tmux::process_fingerprint(old.0),
            Some(old.1.clone())
        );
        drop(runtime);
        assert!(!root.exists());
        assert_ne!(super::super::tmux::process_fingerprint(old.0), Some(old.1));
        assert_ne!(
            super::super::tmux::process_fingerprint(active.0),
            Some(active.1)
        );
    }

    #[test]
    fn panic_cleanup_terminates_owned_pid_even_when_socket_is_unlinked() {
        use crate::agent::TmuxRuntime;
        if !TmuxRuntime::available() {
            return;
        }
        let runtime = TmuxRuntime::isolated("panic-cleanup");
        let path = runtime.socket_path();
        let root = path.parent().unwrap().to_path_buf();
        let server = start_test_server(&path, "panic-cleanup", "test");
        std::fs::remove_file(&path).unwrap();
        let caught = std::panic::catch_unwind(move || {
            let _runtime = runtime;
            panic!("exercise fixture unwinding");
        });
        assert!(caught.is_err());
        assert!(!root.exists());
        assert_ne!(
            super::super::tmux::process_fingerprint(server.0),
            Some(server.1)
        );
    }

    #[test]
    fn dead_legacy_server_migrates_and_reused_pid_is_not_signalled() {
        let ops = Scripted::default();
        let path = legacy_socket("i1-scripted");
        ops.add(path, 42, true);
        let mut state = SocketState::default();
        state.ensure_with(&ops, "i1-scripted", false).unwrap();
        ops.servers.borrow_mut().clear();
        ops.reachable.borrow_mut().clear();
        ops.starts
            .borrow_mut()
            .insert(42, "different-process".into());
        assert_eq!(
            state.ensure_with(&ops, "i1-scripted", true).unwrap(),
            managed_socket("i1-scripted")
        );
        assert!(ops.signals.borrow().is_empty());
    }

    #[test]
    fn real_legacy_server_survives_socket_unlink_and_runtime_adoption() {
        use crate::agent::TmuxRuntime;
        if !TmuxRuntime::available() {
            return;
        }
        let session = format!("i1-legacy-{}", std::process::id());
        let runtime = TmuxRuntime::isolated(&session);
        let path = runtime.test_legacy_socket_path();
        let managed = runtime.socket_path();
        let program = super::super::tmux::tmux_program();
        let output = Command::new(&program)
            .arg("-S")
            .arg(&path)
            .args([
                "new-session",
                "-d",
                "-s",
                &session,
                "-n",
                "lane-1",
                "sleep 60",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let pid = query(&program, &path).expect("isolated legacy pid");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(runtime.list_windows().unwrap(), ["lane-1"]);
        assert_eq!(runtime.socket_path(), path);
        assert_eq!(query(&program, &path), Some(pid));
        assert!(!managed.exists());
        // The adopted runtime remembers the same PID for subsequent socket loss.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(runtime.list_windows().unwrap(), ["lane-1"]);
        assert_eq!(query(&program, &path), Some(pid));
    }

    #[test]
    fn lsof_owners_require_exact_socket_path_and_tmux_command() {
        let output = "p42\nctmux\nf6\nn/private/tmp/tmux-501/i1 (deleted)\np43\ncother\nn/tmp/tmux-501/i1\np44\nctmux\nn/tmp/tmux-501/i1-extra\n";
        assert_eq!(socket_owners(output, Path::new("/tmp/tmux-501/i1")), [42]);
    }
}
