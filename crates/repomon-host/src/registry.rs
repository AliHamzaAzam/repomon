//! Host registry files and pipe naming (PROTOCOL.md §2, §8).
//!
//! The registry directory is the Windows equivalent of tmux's window list: one JSON file per
//! live window under `<data_dir>\hosts\<session>\`, written atomically on startup and removed
//! on exit. The daemon's re-adoption scan (Track I) walks it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One live window's registry file (PROTOCOL.md §8, schema v1). Field order matches the
/// documented example; unknown fields are ignored on read (additive-only evolution).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryEntry {
    pub v: u32,
    pub session: String,
    pub window: String,
    pub pipe: String,
    pub host_pid: u32,
    pub agent_pid: u32,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub owner: String,
    pub started_at: i64,
}

/// `\\.\pipe\repomon-<session>-<window>` (PROTOCOL.md §2).
pub fn pipe_name(session: &str, window: &str) -> String {
    format!(r"\\.\pipe\repomon-{session}-{window}")
}

/// `<data_dir>\hosts\<session>\<window>.json` (PROTOCOL.md §8).
pub fn registry_path(data_dir: &Path, session: &str, window: &str) -> PathBuf {
    data_dir
        .join("hosts")
        .join(session)
        .join(format!("{window}.json"))
}

/// repomon's data dir, mirroring `repomon_core::config::data_dir()` exactly (the host must
/// not depend on the heavy core crate): `REPOMON_DATA_DIR` override, else the platform
/// project data dir.
pub fn data_dir() -> PathBuf {
    data_dir_from(std::env::var("REPOMON_DATA_DIR").ok())
}

fn data_dir_from(override_var: Option<String>) -> PathBuf {
    if let Some(x) = override_var
        && !x.is_empty()
    {
        return PathBuf::from(x);
    }
    directories::ProjectDirs::from("", "", "repomon")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".").join("repomon-data"))
}

/// Write the entry atomically: temp file in the same directory, then rename. Creates parent
/// directories. A crash can leave a `.tmp` file but never a torn JSON.
pub fn write_atomic(path: &Path, entry: &RegistryEntry) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "registry path has no parent",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let json = serde_json::to_vec(entry).expect("registry entry serializes");
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, json)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Remove the entry; already-gone is success (host exit races the daemon's stale GC).
pub fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// A random 128-bit hex owner token, for hosts spawned without `--owner`.
pub fn generate_owner_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("os entropy");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> RegistryEntry {
        RegistryEntry {
            v: 1,
            session: "repomon".into(),
            window: "lane-3-1".into(),
            pipe: pipe_name("repomon", "lane-3-1"),
            host_pid: 4321,
            agent_pid: 5678,
            program: "claude".into(),
            args: vec!["--permission-mode".into(), "plan".into()],
            cwd: "C:\\Users\\me\\code\\proj".into(),
            owner: "daemon-DESKTOP-ME-1a2b3c".into(),
            started_at: 1789000000,
        }
    }

    #[test]
    fn pipe_name_matches_protocol() {
        assert_eq!(
            pipe_name("repomon", "lane-3-1"),
            r"\\.\pipe\repomon-repomon-lane-3-1"
        );
    }

    #[test]
    fn registry_path_matches_protocol() {
        let p = registry_path(std::path::Path::new("/data"), "repomon", "lane-3-1");
        assert_eq!(
            p,
            std::path::Path::new("/data")
                .join("hosts")
                .join("repomon")
                .join("lane-3-1.json")
        );
    }

    #[test]
    fn registry_entry_wire_shape_is_frozen() {
        assert_eq!(
            serde_json::to_string(&entry()).unwrap(),
            r#"{"v":1,"session":"repomon","window":"lane-3-1","pipe":"\\\\.\\pipe\\repomon-repomon-lane-3-1","host_pid":4321,"agent_pid":5678,"program":"claude","args":["--permission-mode","plan"],"cwd":"C:\\Users\\me\\code\\proj","owner":"daemon-DESKTOP-ME-1a2b3c","started_at":1789000000}"#
        );
    }

    #[test]
    fn write_read_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry();
        let path = registry_path(dir.path(), &e.session, &e.window);
        write_atomic(&path, &e).unwrap();

        let back: RegistryEntry = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(back, e);

        // No temp litter next to the file.
        let siblings: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|d| d.unwrap().file_name())
            .collect();
        assert_eq!(siblings, vec![std::ffi::OsString::from("lane-3-1.json")]);

        remove(&path).unwrap();
        assert!(!path.exists());
        remove(&path).unwrap(); // idempotent — a second remove is not an error
    }

    #[test]
    fn data_dir_override_wins() {
        let with = data_dir_from(Some("C:\\override".into()));
        assert_eq!(with, std::path::PathBuf::from("C:\\override"));
        let empty = data_dir_from(Some(String::new()));
        assert_ne!(
            empty,
            std::path::PathBuf::from(""),
            "empty override is ignored"
        );
    }

    #[test]
    fn owner_tokens_are_random_hex() {
        let (a, b) = (generate_owner_token(), generate_owner_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
