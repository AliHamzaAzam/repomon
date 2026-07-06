//! Locating external tools on `$PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// First executable hit for `bin` on `$PATH`, or `None` when it isn't installed.
pub fn find_in_path(bin: &str) -> Option<PathBuf> {
    find_in(std::env::var_os("PATH")?.as_os_str(), bin)
}

fn find_in(path_var: &OsStr, bin: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(bin))
        .find(|cand| is_executable(cand))
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
