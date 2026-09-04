//! Grants the Tauri asset protocol read access to a lane worktree root so the frontend can
//! stream large binary files (PDFs today, and later other non-text assets) straight off disk via
//! `convertFileSrc` instead of round-tripping them through `file.read_raw`'s base64/8 MiB-capped
//! RPC. The daemon has no filesystem-serving RPC of its own for this - it is a webview-native
//! protocol scope grant, so it lives here rather than in `ipc.rs`.

use std::path::Path;

use tauri::{AppHandle, Manager};

/// Rejects anything that isn't an existing directory - a file, a symlink to a file, or a path
/// that doesn't exist yet - since the whole point of scoping to a worktree *root* is that callers
/// only ever grant a directory, never a caller-chosen file.
fn validate_directory(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} is not an existing directory", path.display()));
    }
    Ok(())
}

/// Allow-lists `path` (and everything under it, recursively) for the asset protocol, so an
/// `asset://localhost/<path>` (or `convertFileSrc`-built) URL under it can be loaded by the
/// webview. The frontend calls this once per lane worktree root, the first time a streamed asset
/// is opened in that lane, and keeps its own per-root set so the call isn't repeated - this
/// command itself is idempotent either way.
#[tauri::command]
pub fn allow_worktree_assets(app: AppHandle, path: String) -> Result<(), String> {
    let candidate = Path::new(&path);
    validate_directory(candidate)?;
    app.asset_protocol_scope()
        .allow_directory(candidate, true)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        assert!(validate_directory(&missing).is_err());
    }

    #[test]
    fn rejects_a_path_that_is_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("worktree.txt");
        std::fs::write(&file_path, "not a directory").unwrap();
        assert!(validate_directory(&file_path).is_err());
    }

    #[test]
    fn allows_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(validate_directory(dir.path()).is_ok());
    }
}
