//! Installing the `repomon` command line from inside the app.
//!
//! The bundle already carries every executable the CLI needs: the app, the daemon, and (on
//! Windows) the per-agent host all ship side by side, and this module publishes the two or three
//! a terminal cares about into a directory the user's shell can see. macOS and Linux get symlinks
//! into `~/.local/bin`, so an app update is picked up without reinstalling. Windows gets copies in
//! `%LOCALAPPDATA%\repomon\bin` (a symlink there needs developer mode or an elevated prompt) plus
//! that directory on the *user* PATH. The machine PATH is never touched.
//!
//! Every decision below that can be made without touching the filesystem is a pure function, so
//! the Windows layout is testable on a Mac.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// What a terminal needs on unix: the CLI itself, plus the daemon, because `repomon daemon
/// install` writes a launchd plist or systemd unit that names `repomond` by path.
pub const UNIX_TOOLS: [&str; 2] = ["repomon", "repomond"];

/// The Windows set. The agent host joins them because the daemon spawns it by looking next to
/// itself, so a `repomond.exe` installed alone could start agents nowhere.
pub const WINDOWS_TOOLS: [&str; 3] = ["repomon.exe", "repomond.exe", "repomon-agent-host.exe"];

/// The PATH entry separator for a platform. Passed in rather than read from a constant so the
/// Windows branch of [`path_lists_dir`] can be exercised anywhere.
pub const UNIX_PATH_SEPARATOR: char = ':';
pub const WINDOWS_PATH_SEPARATOR: char = ';';

/// The tools this platform installs.
pub fn tools() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &WINDOWS_TOOLS
    }
    #[cfg(not(windows))]
    {
        &UNIX_TOOLS
    }
}

/// macOS and Linux: `~/.local/bin`, the XDG-blessed per-user bin directory that most shells
/// already put on PATH.
pub fn unix_install_dir(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

/// Windows: `%LOCALAPPDATA%\repomon\bin`. Local rather than roaming, because these are machine
/// specific binaries, and its own directory so uninstalling can empty it without guessing.
pub fn windows_install_dir(local_app_data: &Path) -> PathBuf {
    local_app_data.join("repomon").join("bin")
}

/// Whether `dir` is already an entry in a PATH-style list. Compares entry by entry rather than by
/// substring, so `~/.local/binaries` is not mistaken for `~/.local/bin`, and tolerates the
/// trailing separators and empty entries real PATH values collect.
pub fn path_lists_dir(path_var: &str, dir: &Path, separator: char) -> bool {
    let target = normalize_entry(&dir.to_string_lossy());
    if target.is_empty() {
        return false;
    }
    path_var
        .split(separator)
        .map(normalize_entry)
        .any(|entry| entry == target)
}

/// Trim quotes and trailing separators so two spellings of the same directory compare equal.
fn normalize_entry(entry: &str) -> String {
    let trimmed = entry.trim().trim_matches('"');
    let trimmed = trimmed.trim_end_matches(['/', '\\']);
    trimmed.to_string()
}

/// The exact line to add to a shell rc file. Quoted so a home directory with a space in it works,
/// and prepended so the installed copy wins over an older one further down PATH.
pub fn shell_rc_line(dir: &Path) -> String {
    format!("export PATH=\"{}:$PATH\"", dir.display())
}

/// The user PATH with `dir` prepended, or `None` when it is already there. `None` is the signal
/// to leave the registry alone rather than rewrite an identical value.
pub fn path_with_dir(existing: &str, dir: &Path) -> Option<String> {
    if path_lists_dir(existing, dir, WINDOWS_PATH_SEPARATOR) {
        return None;
    }
    let dir = dir.to_string_lossy();
    if existing.trim().is_empty() {
        return Some(dir.into_owned());
    }
    Some(format!("{dir};{existing}"))
}

/// The user PATH with `dir` removed, or `None` when it was not there. Every other entry keeps its
/// original spelling: this rewrites a value the user may have edited by hand, so it removes one
/// entry rather than normalising the rest.
pub fn path_without_dir(existing: &str, dir: &Path) -> Option<String> {
    if !path_lists_dir(existing, dir, WINDOWS_PATH_SEPARATOR) {
        return None;
    }
    let target = normalize_entry(&dir.to_string_lossy());
    let kept: Vec<&str> = existing
        .split(WINDOWS_PATH_SEPARATOR)
        .filter(|entry| !entry.trim().is_empty() && normalize_entry(entry) != target)
        .collect();
    Some(kept.join(";"))
}

/// What Settings shows about the command line tools, and what Install and Remove return.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CliStatus {
    /// Every tool for this platform is present in the install directory.
    pub installed: bool,
    /// The install directory, shown whether or not anything is in it yet.
    pub dir: String,
    /// Whether a terminal would find that directory. On unix this is the login shell's PATH, not
    /// the app's, because an app launched from the Dock has a stripped one.
    pub on_path: bool,
    /// `repomon --version` from the installed copy. The verification that it works, not just that
    /// a file of the right name exists.
    pub version: Option<String>,
    /// The tools found in the install directory.
    pub tools: Vec<String>,
    /// The tools that should be there and are not.
    pub missing: Vec<String>,
    /// The exact line to add to a shell rc, or the note about signing out on Windows. `None` when
    /// the directory is already on PATH.
    pub path_hint: Option<String>,
}

/// Where the bundled executables live: next to the running app binary. Tauri drops the target
/// triple from a sidecar's name when it bundles, so these are plain `repomond`/`repomon`.
fn bundled_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("could not resolve this app: {e}"))?;
    exe.parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "this app has no containing directory".to_string())
}

/// The install directory for this platform.
pub fn install_dir() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| "LOCALAPPDATA is not set for this session".to_string())?;
        Ok(windows_install_dir(Path::new(&local)))
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| "HOME is not set for this session".to_string())?;
        Ok(unix_install_dir(Path::new(&home)))
    }
}

/// The PATH a terminal on this machine would actually have.
///
/// On macOS an app launched from the Dock inherits `/usr/bin:/bin:/usr/sbin:/sbin`, so answering
/// "is `~/.local/bin` on your PATH" from this process's own environment would tell almost every
/// user "no" whether or not it is true. Ask the login shell instead, the way the daemon's own
/// PATH repair does, and fall back to this process's PATH when there is no shell to ask.
fn user_path() -> String {
    #[cfg(not(windows))]
    if let Some(shell) = std::env::var_os("SHELL") {
        let output = std::process::Command::new(shell)
            .args(["-ilc", "printf %s \"$PATH\""])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return path;
                }
            }
        }
    }
    std::env::var("PATH").unwrap_or_default()
}

fn path_separator() -> char {
    if cfg!(windows) {
        WINDOWS_PATH_SEPARATOR
    } else {
        UNIX_PATH_SEPARATOR
    }
}

/// `repomon --version` from the installed copy, or `None` when it is not there or will not run.
fn installed_version(dir: &Path) -> Option<String> {
    let exe = dir.join(tools()[0]);
    if !exe.exists() {
        return None;
    }
    let output = repomon_core::process::background_command(&exe)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!version.is_empty()).then_some(version)
}

/// Read the install directory and report what is there, without changing anything.
fn read_status(dir: &Path) -> CliStatus {
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for tool in tools() {
        if dir.join(tool).exists() {
            present.push((*tool).to_string());
        } else {
            missing.push((*tool).to_string());
        }
    }
    let installed = missing.is_empty();
    let on_path = path_lists_dir(&user_path(), dir, path_separator());
    CliStatus {
        installed,
        dir: dir.display().to_string(),
        on_path,
        version: installed.then(|| installed_version(dir)).flatten(),
        tools: present,
        missing,
        path_hint: (!on_path).then(|| path_hint(dir)),
    }
}

/// What to tell a user whose shell cannot see the install directory yet.
fn path_hint(dir: &Path) -> String {
    #[cfg(windows)]
    {
        let _ = dir;
        WINDOWS_PATH_HINT.to_string()
    }
    #[cfg(not(windows))]
    {
        format!(
            "Add this line to your shell rc (~/.zshrc or ~/.bashrc): {}",
            shell_rc_line(dir)
        )
    }
}

/// Windows updates the user PATH in the registry and broadcasts the change, but a console that is
/// already open keeps the environment it was started with.
pub const WINDOWS_PATH_HINT: &str = "Open a new terminal to pick up the updated PATH. Windows keeps the old environment in windows that were already open.";

#[tauri::command]
pub fn cli_status() -> Result<CliStatus, String> {
    Ok(read_status(&install_dir()?))
}

#[tauri::command]
pub fn cli_install() -> Result<CliStatus, String> {
    let dir = install_dir()?;
    let source = bundled_dir()?;

    let absent: Vec<String> = tools()
        .iter()
        .filter(|tool| !source.join(tool).exists())
        .map(|tool| (*tool).to_string())
        .collect();
    if !absent.is_empty() {
        return Err(format!(
            "this build does not carry {} next to the app at {}",
            absent.join(", "),
            source.display()
        ));
    }

    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    for tool in tools() {
        let from = source.join(tool);
        let to = dir.join(tool);
        // Replace whatever is there: an older symlink into a previous app location, or a copy
        // from `install.ps1`. Removing first is what makes install idempotent.
        if to.exists() || std::fs::symlink_metadata(&to).is_ok() {
            std::fs::remove_file(&to)
                .map_err(|e| format!("could not replace {}: {e}", to.display()))?;
        }
        link_or_copy(&from, &to)?;
    }

    #[cfg(windows)]
    windows_path::add_to_user_path(&dir)?;

    Ok(read_status(&dir))
}

#[tauri::command]
pub fn cli_uninstall() -> Result<CliStatus, String> {
    let dir = install_dir()?;
    for tool in tools() {
        let path = dir.join(tool);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // On unix the install directory is shared with everything else the user put in
        // `~/.local/bin`, including a `repomon` from `install.sh` or `cargo install`. Only remove
        // what this card created, which is always a symlink. On Windows the directory is ours
        // alone (`install.ps1` uses `%LOCALAPPDATA%\Programs\repomon`), so anything in it goes.
        #[cfg(unix)]
        if !meta.file_type().is_symlink() {
            continue;
        }
        #[cfg(not(unix))]
        let _ = meta;
        std::fs::remove_file(&path)
            .map_err(|e| format!("could not remove {}: {e}", path.display()))?;
    }

    #[cfg(windows)]
    windows_path::remove_from_user_path(&dir)?;

    Ok(read_status(&dir))
}

/// unix links, Windows copies.
///
/// A symlink keeps the CLI in step with the app across updates, which is why unix uses one. On
/// Windows creating a symlink needs developer mode or an elevated prompt, so the app copies
/// instead and a new app version republishes the copies on the next Install.
fn link_or_copy(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to)
            .map_err(|e| format!("could not link {} to {}: {e}", to.display(), from.display()))
    }
    #[cfg(not(unix))]
    {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| format!("could not copy {} to {}: {e}", from.display(), to.display()))
    }
}

/// The user PATH in `HKEY_CURRENT_USER\Environment`. Never `HKEY_LOCAL_MACHINE`: this install is
/// for one user, needs no elevation, and must not change what other accounts on the machine see.
#[cfg(windows)]
mod windows_path {
    use std::path::Path;

    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, RegType};

    use super::{path_with_dir, path_without_dir};

    const ENVIRONMENT: &str = "Environment";
    const PATH_VALUE: &str = "Path";

    fn open() -> Result<RegKey, String> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(ENVIRONMENT, KEY_READ | KEY_SET_VALUE)
            .map_err(|e| format!("could not open HKCU\\{ENVIRONMENT}: {e}"))
    }

    /// The current user PATH and the registry type to write it back as. The value is normally
    /// `REG_EXPAND_SZ` (it can contain `%USERPROFILE%`), and rewriting it as a plain `REG_SZ`
    /// would silently stop those references from expanding.
    fn read(key: &RegKey) -> (String, RegType) {
        match key.get_raw_value(PATH_VALUE) {
            Ok(value) => {
                let text = String::from_utf16_lossy(&to_u16(&value.bytes))
                    .trim_end_matches('\0')
                    .to_string();
                (text, value.vtype)
            }
            Err(_) => (String::new(), RegType::REG_EXPAND_SZ),
        }
    }

    fn to_u16(bytes: &[u8]) -> Vec<u16> {
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect()
    }

    fn write(key: &RegKey, value: &str, vtype: RegType) -> Result<(), String> {
        let bytes: Vec<u8> = value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        key.set_raw_value(PATH_VALUE, &winreg::RegValue { bytes, vtype })
            .map_err(|e| format!("could not write the user PATH: {e}"))?;
        broadcast_environment_change();
        Ok(())
    }

    pub fn add_to_user_path(dir: &Path) -> Result<(), String> {
        let key = open()?;
        let (current, vtype) = read(&key);
        match path_with_dir(&current, dir) {
            Some(updated) => write(&key, &updated, vtype),
            None => Ok(()),
        }
    }

    pub fn remove_from_user_path(dir: &Path) -> Result<(), String> {
        let key = open()?;
        let (current, vtype) = read(&key);
        match path_without_dir(&current, dir) {
            Some(updated) => write(&key, &updated, vtype),
            None => Ok(()),
        }
    }

    /// Tell every top level window the environment changed, so Explorer (and therefore every
    /// terminal started from it afterwards) picks up the new PATH without a sign-out. Windows that
    /// are already open keep the environment they started with either way, which is what
    /// `WINDOWS_PATH_HINT` says. Best effort: a hung window must not fail the install, hence the
    /// timeout and the ignored result.
    fn broadcast_environment_change() {
        use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
        };

        let target: Vec<u16> = ENVIRONMENT
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0 as WPARAM,
                target.as_ptr() as LPARAM,
                SMTO_ABORTIFHUNG,
                5_000,
                std::ptr::null_mut(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_installs_into_the_xdg_user_bin() {
        assert_eq!(
            unix_install_dir(Path::new("/Users/pat")),
            PathBuf::from("/Users/pat/.local/bin")
        );
    }

    #[test]
    fn windows_installs_into_its_own_local_appdata_directory() {
        // Compared by components, not by spelling: the separator this test runs on is the host's,
        // and what matters is that the two segments hang off LOCALAPPDATA in that order.
        let root = Path::new(r"C:\Users\azama\AppData\Local");
        let dir = windows_install_dir(root);
        assert!(dir.ends_with("repomon/bin"), "got {}", dir.display());
        assert_eq!(dir.parent().and_then(Path::parent), Some(root));
    }

    #[test]
    fn unix_publishes_the_cli_and_the_daemon() {
        // `repomon daemon install` writes a service file naming repomond by path, so the daemon
        // has to be published alongside the CLI or that command installs a broken unit.
        assert_eq!(UNIX_TOOLS, ["repomon", "repomond"]);
    }

    #[test]
    fn windows_publishes_the_agent_host_too() {
        // The daemon spawns the host by looking next to itself.
        assert_eq!(
            WINDOWS_TOOLS,
            ["repomon.exe", "repomond.exe", "repomon-agent-host.exe"]
        );
    }

    #[test]
    fn path_membership_is_by_entry_not_substring() {
        let dir = Path::new("/Users/pat/.local/bin");
        assert!(path_lists_dir(
            "/usr/bin:/Users/pat/.local/bin:/bin",
            dir,
            UNIX_PATH_SEPARATOR
        ));
        assert!(!path_lists_dir(
            "/usr/bin:/Users/pat/.local/binaries:/bin",
            dir,
            UNIX_PATH_SEPARATOR
        ));
        assert!(!path_lists_dir("", dir, UNIX_PATH_SEPARATOR));
    }

    #[test]
    fn path_membership_tolerates_trailing_separators_and_quotes() {
        let dir = Path::new(r"C:\Users\azama\AppData\Local\repomon\bin");
        assert!(path_lists_dir(
            r"C:\Windows;C:\Users\azama\AppData\Local\repomon\bin\;",
            dir,
            WINDOWS_PATH_SEPARATOR
        ));
        assert!(path_lists_dir(
            "\"C:\\Users\\azama\\AppData\\Local\\repomon\\bin\";C:\\Windows",
            dir,
            WINDOWS_PATH_SEPARATOR
        ));
    }

    #[test]
    fn the_rc_line_quotes_the_directory_and_wins_over_older_copies() {
        assert_eq!(
            shell_rc_line(Path::new("/Users/pat with space/.local/bin")),
            "export PATH=\"/Users/pat with space/.local/bin:$PATH\""
        );
    }

    #[test]
    fn the_user_path_gains_the_directory_once() {
        let dir = Path::new(r"C:\Users\azama\AppData\Local\repomon\bin");
        let updated = path_with_dir(r"C:\Windows;C:\Windows\System32", dir).unwrap();
        assert_eq!(
            updated,
            r"C:\Users\azama\AppData\Local\repomon\bin;C:\Windows;C:\Windows\System32"
        );
        // Already there: leave the registry alone rather than rewrite an identical value.
        assert!(path_with_dir(&updated, dir).is_none());
    }

    #[test]
    fn an_empty_user_path_becomes_just_the_directory() {
        let dir = Path::new(r"C:\Users\azama\AppData\Local\repomon\bin");
        assert_eq!(
            path_with_dir("   ", dir).unwrap(),
            r"C:\Users\azama\AppData\Local\repomon\bin"
        );
    }

    #[test]
    fn removal_takes_one_entry_and_leaves_the_others_spelled_as_they_were() {
        let dir = Path::new(r"C:\Users\azama\AppData\Local\repomon\bin");
        let existing =
            r"C:\Windows;C:\Users\azama\AppData\Local\repomon\bin\;%USERPROFILE%\.cargo\bin";
        assert_eq!(
            path_without_dir(existing, dir).unwrap(),
            r"C:\Windows;%USERPROFILE%\.cargo\bin"
        );
        assert!(path_without_dir(r"C:\Windows", dir).is_none());
    }

    #[test]
    fn the_windows_hint_explains_why_an_open_terminal_still_cannot_see_it() {
        assert!(WINDOWS_PATH_HINT.contains("new terminal"));
    }
}
