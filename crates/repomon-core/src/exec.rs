//! Locating external tools on `$PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// First executable hit for `bin` on `$PATH`, or `None` when it isn't installed.
pub fn find_in_path(bin: &str) -> Option<PathBuf> {
    find_in(std::env::var_os("PATH")?.as_os_str(), bin)
}

#[cfg(not(windows))]
fn find_in(path_var: &OsStr, bin: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(bin))
        .find(|cand| is_executable(cand))
}

/// Windows: executables are found by extension, per `PATHEXT` — `claude` is really
/// `claude.cmd` (the npm shim), `git` is `git.exe`, `wt` is `wt.exe`. Each PATH entry is
/// probed with every candidate name before moving on, so the first PATH entry that has the
/// tool wins (matching the unix behavior and `CreateProcess` semantics).
#[cfg(windows)]
fn find_in(path_var: &OsStr, bin: &str) -> Option<PathBuf> {
    let pathext = std::env::var("PATHEXT").ok().filter(|v| !v.is_empty());
    let names = candidate_names(bin, pathext.as_deref().unwrap_or(DEFAULT_PATHEXT));
    std::env::split_paths(path_var)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|cand| is_executable(cand))
}

/// The stock Windows default, used when `PATHEXT` is unset/empty.
#[cfg(any(windows, test))]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// The filenames to try for `bin` under a `PATHEXT` value, in order. A name that already
/// carries an extension is tried as given first (like `CreateProcess`); an extensionless name
/// tries each PATHEXT extension first and the bare name last (best-effort for extensionless
/// scripts, and harmless because real Windows tools always match an extension earlier).
/// Pure string logic so it is unit-testable on every OS.
#[cfg(any(windows, test))]
fn candidate_names(bin: &str, pathext: &str) -> Vec<String> {
    let mut names = Vec::new();
    let has_ext = std::path::Path::new(bin).extension().is_some();
    if has_ext {
        names.push(bin.to_string());
    }
    for ext in pathext.split(';') {
        let ext = ext.trim();
        if !ext.is_empty() {
            names.push(format!("{bin}{ext}"));
        }
    }
    if !has_ext {
        names.push(bin.to_string());
    }
    names
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn finds_executable_on_synthetic_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("sometool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        make_executable(&tool);
        let path_var = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_in(&path_var, "sometool"), Some(tool));
        assert_eq!(find_in(&path_var, "missing"), None);
    }

    #[cfg(unix)]
    #[test]
    fn skips_non_executable_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("plainfile"), "data").unwrap();
        let path_var = std::env::join_paths([dir.path()]).unwrap();
        assert_eq!(find_in(&path_var, "plainfile"), None);
    }

    #[test]
    fn pathext_candidates_prefer_extensions_for_bare_names() {
        assert_eq!(
            candidate_names("claude", DEFAULT_PATHEXT),
            vec![
                "claude.COM",
                "claude.EXE",
                "claude.BAT",
                "claude.CMD",
                "claude"
            ]
        );
        // An explicit extension is honored as given, first.
        assert_eq!(
            candidate_names("tool.exe", ".COM;.EXE"),
            vec!["tool.exe", "tool.exe.COM", "tool.exe.EXE"]
        );
        // Empty PATHEXT segments are skipped.
        assert_eq!(candidate_names("x", ".EXE;;"), vec!["x.EXE", "x"]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_find_in_resolves_a_cmd_shim() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("shim.cmd"), "@echo off\r\n").unwrap();
        let path_var = std::env::join_paths([dir.path()]).unwrap();
        // PATHEXT candidates carry the list's (upper)case; NTFS matches case-insensitively,
        // so compare the same way instead of pinning the extension's case.
        let found = find_in(&path_var, "shim").expect("shim.cmd is found via PATHEXT");
        let want = dir.path().join("shim.cmd");
        assert!(
            found
                .to_string_lossy()
                .eq_ignore_ascii_case(&want.to_string_lossy()),
            "found {found:?}, want (case-insensitive) {want:?}"
        );
    }

    #[test]
    fn first_path_entry_wins() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        for dir in [&a, &b] {
            let tool = dir.path().join("dup");
            std::fs::write(&tool, "#!/bin/sh\n").unwrap();
            #[cfg(unix)]
            make_executable(&tool);
        }
        let path_var = std::env::join_paths([a.path(), b.path()]).unwrap();
        assert_eq!(find_in(&path_var, "dup"), Some(a.path().join("dup")));
    }
}
