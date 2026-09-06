//! Grants worktree access to Tauri’s asset protocol so large binary viewers can stream files
//! without the capped base64 RPC.

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

/// Idempotently grants recursive asset-protocol access to a worktree root for streamed file
/// previews.
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
