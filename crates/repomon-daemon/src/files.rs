//! Worktree-scoped file I/O for the upcoming in-app file editor.
//!
//! `file.list` (D1) lists one directory level of a lane's worktree; `file.read`/`file.write`
//! (D2) read and atomically write individual files within it. All three are **local-only** — see
//! `remote::remote_method_allowed`'s doc comment, which withholds them for the same reason it
//! already withholds `fs.browse`, doubly so now that `file.write` touches the host filesystem.
//!
//! Path handling mirrors `ext::skill_path_allowed`: every caller-supplied path is resolved
//! through [`worktree_path_allowed`] before any filesystem call, which canonicalizes both the
//! worktree root and the candidate (via `ext::canonical_prefix`, symlink-safe and tolerant of a
//! not-yet-existing tail) and requires strict prefix containment.

use std::collections::HashSet;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use repomon_core::model::{FileEntry, FileListResult, FileReadResult, FileWriteResult};
use repomon_core::process::background_command;

use crate::ext::canonical_prefix;

/// Cap on entries returned by one `file.list` call. A directory level, not a recursive tree, but
/// still needs a backstop (e.g. a `node_modules` targeted directly).
pub const LIST_CAP: usize = 2000;

/// Cap on `file.read`'s content size. Past this the RPC rejects rather than truncates — see
/// [`ReadError::TooLarge`].
pub const READ_CAP_BYTES: u64 = 2 * 1024 * 1024;

/// How many leading bytes to sniff for a null byte when classifying binary vs. text.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Resolve `rel` (a caller-supplied path, relative to `root`) to an absolute path guaranteed to
/// live inside `root`, or `None` if it doesn't. `rel = ""` resolves to `root` itself (used for a
/// `file.list` with no `path`). Two layers, both required:
///
/// 1. Reject an absolute `rel`, or one with a literal `..` component, before joining anything.
///    `PathBuf::join` silently **discards the base** when handed an absolute second argument, so
///    skipping this would let a caller-supplied `/etc/passwd` sail straight through the
///    containment check below unchanged.
/// 2. Canonicalize both `root` and the joined candidate (`ext::canonical_prefix`, which
///    tolerates a not-yet-existing tail — needed because `file.write` may be creating a new
///    file, and per the task this also covers "canonicalize the parent" since the walk stops at
///    the nearest existing ancestor, normally the parent directory) and require the target's
///    resolution to literally start with the root's. This is what actually catches a symlink
///    escape (a committed symlink inside the worktree pointing outside it), which layer 1 can't
///    see.
pub fn worktree_path_allowed(root: &Path, rel: &str) -> Option<PathBuf> {
    let candidate = Path::new(rel);
    if candidate.is_absolute()
        || candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return None;
    }
    let joined = root.join(candidate);
    let root_resolved = canonical_prefix(root)?;
    let target_resolved = canonical_prefix(&joined)?;
    if target_resolved.starts_with(&root_resolved) {
        Some(joined)
    } else {
        None
    }
}

/// One level of `dir` (assumed already validated by [`worktree_path_allowed`] and inside `root`),
/// gitignore-aware and capped at [`LIST_CAP`]. Sorted directories-first, then case-insensitively
/// by name — same convention as `browse_dir`.
///
/// **Gitignore approach:** `repomon-core`'s only gix status walk (`reader::dirty_state`) iterates
/// tracked/changed files across the whole tree via `gix::status()` — it isn't a one-level
/// directory lister and by default excludes ignored entries entirely rather than flagging them,
/// so reusing it here isn't straightforward. Instead this always excludes `.git` (independent of
/// gitignore) and classifies the rest with one batched `git check-ignore --stdin` per listing
/// (see [`check_ignored`]) rather than a shell-out per entry.
pub fn list_dir(root: &Path, dir: &Path) -> io::Result<FileListResult> {
    let mut raw = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue; // skip non-UTF8 names; nothing in FileEntry could represent them anyway
        };
        if name == ".git" {
            continue; // always excluded, independent of gitignore
        }
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let size = if is_dir {
            None
        } else {
            std::fs::metadata(&path).ok().map(|m| m.len())
        };
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        raw.push((name, rel, is_dir, size));
    }
    raw.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let truncated = raw.len() > LIST_CAP;
    raw.truncate(LIST_CAP);

    let rel_paths: Vec<String> = raw.iter().map(|(_, rel, ..)| rel.clone()).collect();
    let ignored = check_ignored(root, &rel_paths);
    let entries = raw
        .into_iter()
        .map(|(name, rel, is_dir, size)| FileEntry {
            ignored: ignored.contains(&rel),
            name,
            path: rel,
            is_dir,
            size,
        })
        .collect();
    Ok(FileListResult { entries, truncated })
}

/// Batch-classify already-collected worktree-relative paths as git-ignored via one
/// `git check-ignore --stdin`, instead of a shell-out per entry. Best-effort: any failure (git
/// missing, `root` not a git repo, non-UTF8 output, ...) reports nothing ignored. This flag is
/// informational (dims an entry in the tree) rather than a security boundary — the containment
/// check in [`worktree_path_allowed`] is the actual boundary — so failing open here can't expose
/// anything that check wouldn't already gate.
fn check_ignored(root: &Path, rel_paths: &[String]) -> HashSet<String> {
    use std::io::Write;
    use std::process::Stdio;

    if rel_paths.is_empty() {
        return HashSet::new();
    }
    let mut child = match background_command("git")
        .arg("-C")
        .arg(root)
        .args(["check-ignore", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return HashSet::new(),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let payload = rel_paths.join("\n");
        let _ = stdin.write_all(payload.as_bytes());
        // `stdin` drops here, closing the pipe so `git check-ignore` sees EOF and exits.
    }
    let Ok(out) = child.wait_with_output() else {
        return HashSet::new();
    };
    // Exit code 1 means "nothing matched" (not a failure, an empty result); a higher exit code
    // is a real error, but any stdout it did produce is still safe to use, for the same
    // fail-open reasoning as above.
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// Why `read_file` refused to hand back a file's content.
#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    /// A null byte in the first [`BINARY_SNIFF_BYTES`], or content that isn't valid UTF-8.
    Binary,
    /// Larger than [`READ_CAP_BYTES`]. Carries the actual size for the error message.
    TooLarge(u64),
}
impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// Read a worktree file for the editor. Deliberately does NOT truncate on either binary content
/// or an oversized file — both are hard rejections. This differs on purpose from display RPCs
/// like `lane.diff`'s patch (cap + `_truncated` flag): a diff is read-only display, but a
/// truncated `file.read` risks the editor round-tripping the truncated copy back over the real
/// file on save, silently destroying the tail the user never saw.
pub fn read_file(path: &Path) -> Result<FileReadResult, ReadError> {
    let meta = std::fs::metadata(path)?;
    let size = meta.len();
    if size > READ_CAP_BYTES {
        return Err(ReadError::TooLarge(size));
    }
    let bytes = std::fs::read(path)?;
    let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
    if bytes[..sniff_len].contains(&0u8) {
        return Err(ReadError::Binary);
    }
    let content = String::from_utf8(bytes).map_err(|_| ReadError::Binary)?;
    let mtime_ms = mtime_ms_of(&meta)?;
    Ok(FileReadResult {
        content,
        mtime_ms,
        size,
        truncated: false,
    })
}

/// Why `write_file` refused to write.
#[derive(Debug)]
pub enum WriteError {
    Io(io::Error),
    /// `expected_mtime_ms` was given and didn't match what's on disk (`actual_ms = None` when
    /// the file no longer exists at all — also a conflict, not a fresh create, since the caller
    /// believed it existed).
    Conflict {
        expected_ms: u64,
        actual_ms: Option<u64>,
    },
    /// The parent directory doesn't exist. v1 doesn't `mkdir -p`.
    NoParentDir,
}
impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        WriteError::Io(e)
    }
}

/// Write a worktree file atomically (sibling temp file + rename — the same pattern
/// `ensure_antigravity_mcp_registration` uses for config saves), optionally guarded by an
/// optimistic-concurrency check against the mtime the editor last read.
pub fn write_file(
    path: &Path,
    content: &str,
    expected_mtime_ms: Option<u64>,
) -> Result<FileWriteResult, WriteError> {
    if let Some(expected) = expected_mtime_ms {
        match std::fs::metadata(path) {
            Ok(meta) => {
                let actual = mtime_ms_of(&meta)?;
                if actual != expected {
                    return Err(WriteError::Conflict {
                        expected_ms: expected,
                        actual_ms: Some(actual),
                    });
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(WriteError::Conflict {
                    expected_ms: expected,
                    actual_ms: None,
                });
            }
            Err(e) => return Err(e.into()),
        }
    }
    let parent = path.parent().ok_or(WriteError::NoParentDir)?;
    if !parent.is_dir() {
        return Err(WriteError::NoParentDir);
    }
    let file_name = path.file_name().ok_or(WriteError::NoParentDir)?;
    let tmp_path = parent.join(format!("{}.repomon-tmp", file_name.to_string_lossy()));
    std::fs::write(&tmp_path, content.as_bytes())?;
    std::fs::rename(&tmp_path, path)?;
    let meta = std::fs::metadata(path)?;
    Ok(FileWriteResult {
        mtime_ms: mtime_ms_of(&meta)?,
        size: meta.len(),
    })
}

fn mtime_ms_of(meta: &std::fs::Metadata) -> io::Result<u64> {
    let modified = meta.modified()?;
    let dur = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    Ok(dur.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_a_plain_relative_path_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        assert_eq!(
            worktree_path_allowed(root.path(), "src/main.rs"),
            Some(root.path().join("src/main.rs"))
        );
    }

    #[test]
    fn empty_path_resolves_to_the_root_itself() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            worktree_path_allowed(root.path(), ""),
            Some(root.path().to_path_buf())
        );
    }

    #[test]
    fn allows_a_not_yet_existing_leaf_whose_parent_exists() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            worktree_path_allowed(root.path(), "new-file.txt"),
            Some(root.path().join("new-file.txt"))
        );
    }

    #[test]
    fn rejects_literal_parent_dir_components() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(worktree_path_allowed(root.path(), "../escape.txt"), None);
        assert_eq!(
            worktree_path_allowed(root.path(), "src/../../escape.txt"),
            None
        );
    }

    #[test]
    fn rejects_an_absolute_path() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(worktree_path_allowed(root.path(), "/etc/passwd"), None);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_that_resolves_outside_the_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "shh").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("link"))
            .unwrap();
        assert_eq!(worktree_path_allowed(root.path(), "link"), None);
    }

    #[test]
    fn read_file_rejects_null_byte_content_as_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob.bin");
        std::fs::write(&path, [b'a', b'b', 0u8, b'c']).unwrap();
        assert!(matches!(read_file(&path), Err(ReadError::Binary)));
    }

    #[test]
    fn read_file_rejects_over_cap_size_without_truncating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.txt");
        std::fs::write(&path, vec![b'x'; (READ_CAP_BYTES + 1) as usize]).unwrap();
        match read_file(&path) {
            Err(ReadError::TooLarge(size)) => assert_eq!(size, READ_CAP_BYTES + 1),
            other => panic!("expected TooLarge, got is_ok={}", other.is_ok()),
        }
    }

    #[test]
    fn write_then_read_round_trips_content_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        let written = write_file(&path, "hello\n", None).unwrap();
        let read = read_file(&path).unwrap();
        assert_eq!(read.content, "hello\n");
        assert_eq!(read.mtime_ms, written.mtime_ms);
        assert_eq!(read.size, written.size);
        // Atomic write leaves no temp file behind.
        assert!(!dir.path().join("note.txt.repomon-tmp").exists());
    }

    #[test]
    fn write_rejects_stale_expected_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("note.txt");
        let written = write_file(&path, "v1", None).unwrap();
        match write_file(&path, "v2", Some(written.mtime_ms.wrapping_sub(1))) {
            Err(WriteError::Conflict {
                expected_ms,
                actual_ms,
            }) => {
                assert_eq!(expected_ms, written.mtime_ms.wrapping_sub(1));
                assert_eq!(actual_ms, Some(written.mtime_ms));
            }
            other => panic!("expected Conflict, got is_ok={}", other.is_ok()),
        }
        // Rejected write must not have touched the file.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v1");
    }

    #[test]
    fn write_rejects_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope/note.txt");
        assert!(matches!(
            write_file(&path, "x", None),
            Err(WriteError::NoParentDir)
        ));
    }
}
