//! PATH repair for daemons started from a GUI.
//!
//! macOS gives an app launched from Finder or the Dock a stripped `PATH`
//! (`/usr/bin:/bin:/usr/sbin:/sbin`), and every process it spawns inherits it. A daemon started
//! that way cannot see `tmux` (typically `/opt/homebrew/bin`), `claude` (`~/.local/bin`), or
//! `codex`, so agent detection reports everything missing and a spawn fails with a bare
//! `No such file or directory`. The same applies to a launchd service, whose plist carries no
//! environment of its own.
//!
//! The repair only runs when `PATH` is exactly the stripped default, so a daemon started from a
//! shell (or one given a deliberate `PATH`) is never second-guessed.

use std::path::PathBuf;
use std::process::Command;

/// The environment a Finder/Dock launch provides. Matched exactly: anything else is treated as a
/// PATH somebody meant, not one to override.
const MINIMAL_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// Directories worth adding when the login shell cannot be consulted. Covers Homebrew on both
/// Apple silicon and Intel plus the usual per-user tool dirs.
const FALLBACK_ABSOLUTE: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];
const FALLBACK_HOME_RELATIVE: [&str; 4] = [".local/bin", ".cargo/bin", ".bun/bin", "bin"];

/// True when `path` is the stripped default a GUI launch hands down.
pub fn looks_minimal(path: &str) -> bool {
    path.trim() == MINIMAL_PATH
}

/// Ask the user's login shell what `PATH` should be. `-i` matters: many setups export `PATH` from
/// an interactive rc file (`.zshrc`) rather than a login profile, so a non-interactive shell would
/// answer with the same stripped value we are trying to replace.
fn login_shell_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok()?;
    let output = Command::new(shell)
        .args(["-ilc", "printf %s \"$PATH\""])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!path.is_empty() && !looks_minimal(&path)).then_some(path)
}

/// Existing well-known tool directories not already on `path`, in the order they should be added.
fn fallback_dirs(path: &str) -> Vec<String> {
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    let candidates = FALLBACK_ABSOLUTE
        .iter()
        .map(PathBuf::from)
        .chain(FALLBACK_HOME_RELATIVE.iter().filter_map(|rel| {
            home.as_ref().map(|home| home.join(rel))
        }));
    let mut dirs = Vec::new();
    for dir in candidates {
        if !dir.is_dir() {
            continue;
        }
        let entry = dir.display().to_string();
        if path.split(':').any(|existing| existing == entry) || dirs.contains(&entry) {
            continue;
        }
        dirs.push(entry);
    }
    dirs
}

/// The `PATH` this process should run with, or `None` when the current one is already usable.
pub fn repaired_path() -> Option<String> {
    let current = std::env::var("PATH").unwrap_or_default();
    if !looks_minimal(&current) {
        return None;
    }
    if let Some(path) = login_shell_path() {
        return Some(path);
    }
    let extra = fallback_dirs(&current);
    (!extra.is_empty()).then(|| format!("{}:{current}", extra.join(":")))
}

/// Repair `PATH` in this process so everything the daemon spawns can find the user's tools.
///
/// # Safety
///
/// Call before starting the async runtime or any other thread: [`std::env::set_var`] is only sound
/// while the process is single-threaded.
pub unsafe fn repair_path_before_threads() {
    if let Some(path) = repaired_path() {
        tracing::debug!(%path, "restored the user PATH for a GUI-started daemon");
        unsafe { std::env::set_var("PATH", path) };
    }
}

#[cfg(test)]
mod tests {
    use super::{fallback_dirs, looks_minimal};

    #[test]
    fn only_the_stripped_gui_default_counts_as_minimal() {
        assert!(looks_minimal("/usr/bin:/bin:/usr/sbin:/sbin"));
        assert!(looks_minimal("  /usr/bin:/bin:/usr/sbin:/sbin  "));
        // A real user PATH, or any deliberate one, is left alone.
        assert!(!looks_minimal("/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin"));
        assert!(!looks_minimal("/usr/bin:/bin"));
        assert!(!looks_minimal(""));
    }

    #[test]
    fn fallbacks_skip_directories_already_on_path() {
        // Whatever the host actually has, nothing already present may be suggested again, and no
        // entry may repeat (a duplicate would shadow later lookups for no benefit).
        let dirs = fallback_dirs("/usr/bin:/bin:/usr/sbin:/sbin");
        for dir in &dirs {
            assert!(!"/usr/bin:/bin:/usr/sbin:/sbin".split(':').any(|p| p == dir));
        }
        let mut seen = dirs.clone();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), dirs.len());

        // A PATH that already contains a candidate does not get it back.
        let with_brew = fallback_dirs("/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin");
        assert!(!with_brew.iter().any(|d| d == "/opt/homebrew/bin"));
    }
}
