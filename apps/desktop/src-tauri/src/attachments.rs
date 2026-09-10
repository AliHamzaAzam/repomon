//! Keeps pasted attachments outside the worktree and alive after the composer is cleared.
use std::{io::Write, path::Path};
use tauri::Manager;

const MAX_BYTES: usize = 20 * 1024 * 1024;

fn persist(root: &Path, name: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty() || bytes.len() > MAX_BYTES {
        return Err("Choose a nonempty attachment smaller than 20 MB.".into());
    }
    // No caller-controlled directories. Keep an extension so the receiving agent recognizes
    // images, and create exclusively so simultaneous pastes cannot overwrite one another.
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(100)
        .collect();
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut file = tempfile::Builder::new()
        .prefix("attachment-")
        .suffix(&format!("-{name}"))
        .tempfile_in(root)
        .map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    let (_, path) = file.keep().map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn save_chat_attachment(
    app: tauri::AppHandle,
    name: String,
    bytes: Vec<u8>,
) -> Result<String, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("attachments");
    tauri::async_runtime::spawn_blocking(move || persist(&root, &name, &bytes))
        .await
        .map_err(|e| e.to_string())?
}

fn preview_file(path: &Path) -> Result<std::path::PathBuf, String> {
    let canonical = path.canonicalize().map_err(|_| "Attachment unavailable")?;
    let meta = canonical.metadata().map_err(|_| "Attachment unavailable")?;
    if !meta.is_file() || meta.len() > MAX_BYTES as u64 {
        return Err("Attachment preview unavailable".into());
    }
    Ok(canonical)
}

/// Grant only the requested file, never its parent directory. The asset protocol
/// streams image bytes without embedding filesystem paths into transcript prose.
#[tauri::command]
pub async fn allow_chat_attachment_preview(
    app: tauri::AppHandle,
    path: String,
) -> Result<String, String> {
    let path = tauri::async_runtime::spawn_blocking(move || preview_file(Path::new(&path)))
        .await
        .map_err(|_| "Attachment unavailable")??;
    app.asset_protocol_scope()
        .allow_file(&path)
        .map_err(|_| "Attachment unavailable")?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_bytes_and_extension_without_traversal_or_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let first = persist(dir.path(), "../../image.png", b"image bytes").unwrap();
        let second = persist(dir.path(), "../../image.png", b"other image").unwrap();
        assert_ne!(first, second);
        assert_eq!(Path::new(&first).parent(), Some(dir.path()));
        assert!(first.ends_with(".png"));
        assert_eq!(std::fs::read(first).unwrap(), b"image bytes");
    }
    #[test]
    fn preview_requires_an_existing_bounded_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = persist(dir.path(), "image.png", b"image bytes").unwrap();
        assert_eq!(
            preview_file(Path::new(&file)).unwrap(),
            Path::new(&file).canonicalize().unwrap()
        );
        assert!(preview_file(dir.path()).is_err());
        assert!(preview_file(&dir.path().join("missing.png")).is_err());
        let large = dir.path().join("large.png");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(preview_file(&large).is_err());
    }
    #[test]
    fn rejects_empty_and_oversized_attachments() {
        let dir = tempfile::tempdir().unwrap();
        assert!(persist(dir.path(), "empty.png", &[]).is_err());
        assert!(persist(dir.path(), "big.png", &vec![0; MAX_BYTES + 1]).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
