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

use base64::Engine;
use repomon_core::model::{
    FileCreateResult, FileDeleteResult, FileDiffBaseResult, FileEntry, FileListResult,
    FileReadRawResult, FileReadResult, FileRenameResult, FileSearchHit, FileSearchResult,
    FileWriteResult,
};
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
pub(crate) fn check_ignored(root: &Path, rel_paths: &[String]) -> HashSet<String> {
    use std::io::Write;
    use std::process::Stdio;

    if rel_paths.is_empty() {
        return HashSet::new();
    }
    let mut result = HashSet::new();
    for chunk in rel_paths.chunks(1000) {
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
            Err(_) => continue,
        };
        if let Some(mut stdin) = child.stdin.take() {
            let payload = chunk.join("\n");
            let _ = stdin.write_all(payload.as_bytes());
            // `stdin` drops here, closing the pipe so `git check-ignore` sees EOF and exits.
        }
        let Ok(out) = child.wait_with_output() else {
            continue;
        };
        // Exit code 1 means "nothing matched" (not a failure, an empty result); a higher exit code
        // is a real error, but any stdout it did produce is still safe to use, for the same
        // fail-open reasoning as above.
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            result.insert(line.to_string());
        }
    }
    result
}

/// Cap on recursive file indexing for `file.index`. Past this the walk stops and sets truncated.
pub const INDEX_CAP: usize = 50_000;

/// Recursively index non-ignored files within `root` (relative paths with `/` separator).
/// Skips `.git` and gitignored directories (e.g. `node_modules`, `target`) without descending into them.
pub fn index_worktree(root: &Path) -> io::Result<(Vec<String>, bool)> {
    let mut files: Vec<String> = Vec::new();
    let mut current_dirs = vec![root.to_path_buf()];
    let mut truncated = false;

    while !current_dirs.is_empty() && !truncated {
        let mut next_dirs = Vec::new();
        let mut next_dir_rels = Vec::new();

        for dir in current_dirs {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                if name == ".git" {
                    continue;
                }
                let path = entry.path();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let rel = match path.strip_prefix(root) {
                    Ok(p) => p.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };

                if is_dir {
                    let dir_rel = format!("{rel}/");
                    next_dirs.push(path);
                    next_dir_rels.push(dir_rel);
                } else {
                    files.push(rel);
                    if files.len() >= INDEX_CAP {
                        truncated = true;
                        break;
                    }
                }
            }
            if truncated {
                break;
            }
        }

        if truncated || next_dirs.is_empty() {
            break;
        }

        // Batch-prune ignored directories so we never descend into them.
        let ignored_dirs = check_ignored(root, &next_dir_rels);
        current_dirs = next_dirs
            .into_iter()
            .zip(next_dir_rels.into_iter())
            .filter_map(|(d, rel)| {
                if ignored_dirs.contains(&rel) {
                    None
                } else {
                    Some(d)
                }
            })
            .collect();
    }

    // Filter collected files for gitignored files (e.g. *.log, secret files)
    let ignored_files = check_ignored(root, &files);
    let mut valid_paths: Vec<String> = files
        .into_iter()
        .filter(|rel| !ignored_files.contains(rel))
        .collect();

    valid_paths.sort();
    Ok((valid_paths, truncated))
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

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn mime_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Read a worktree file for the editor. Detects text, binary, and image files.
/// Deliberately does NOT truncate on oversized file: oversized files are hard rejections.
pub fn read_file(path: &Path) -> Result<FileReadResult, ReadError> {
    let meta = std::fs::metadata(path)?;
    let size = meta.len();
    if size > READ_CAP_BYTES {
        return Err(ReadError::TooLarge(size));
    }
    let mtime_ms = mtime_ms_of(&meta)?;

    if is_image_path(path) {
        return Ok(FileReadResult {
            content: String::new(),
            mtime_ms,
            size,
            truncated: false,
            kind: "image".to_string(),
        });
    }

    let bytes = std::fs::read(path)?;
    let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
    if bytes[..sniff_len].contains(&0u8) {
        return Ok(FileReadResult {
            content: String::new(),
            mtime_ms,
            size,
            truncated: false,
            kind: "binary".to_string(),
        });
    }

    match String::from_utf8(bytes) {
        Ok(content) => Ok(FileReadResult {
            content,
            mtime_ms,
            size,
            truncated: false,
            kind: "text".to_string(),
        }),
        Err(_) => Ok(FileReadResult {
            content: String::new(),
            mtime_ms,
            size,
            truncated: false,
            kind: "binary".to_string(),
        }),
    }
}

/// Read raw file bytes, returning base64 payload and MIME type under the same 2 MiB cap.
pub fn read_file_raw(path: &Path) -> Result<FileReadRawResult, ReadError> {
    let meta = std::fs::metadata(path)?;
    let size = meta.len();
    if size > READ_CAP_BYTES {
        return Err(ReadError::TooLarge(size));
    }
    let data = std::fs::read(path)?;
    let mime = mime_for_path(path).to_string();
    let base64 = base64::engine::general_purpose::STANDARD.encode(&data);
    Ok(FileReadRawResult {
        base64,
        mime,
        size,
    })
}

/// Read the HEAD version of a worktree path for git diff and editor gutter markers.
/// Executes `git show HEAD:<path>` in the lane worktree at `root`.
/// Returns `missing` for untracked or newly added files, `binary` by null-byte sniff (or image extension),
/// and `text` for valid UTF-8, capped at READ_CAP_BYTES.
pub fn diff_base(root: &Path, rel: &str) -> Result<FileDiffBaseResult, ReadError> {
    let norm_rel = rel.trim_start_matches("./").trim_start_matches('/');
    if is_image_path(Path::new(norm_rel)) {
        return Ok(FileDiffBaseResult {
            content: None,
            kind: "binary".to_string(),
        });
    }

    let arg = format!("HEAD:{norm_rel}");
    let child = background_command("git")
        .arg("-C")
        .arg(root)
        .args(["show", &arg])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Ok(FileDiffBaseResult {
            content: None,
            kind: "missing".to_string(),
        });
    }

    let bytes = out.stdout;
    let size = bytes.len() as u64;
    if size > READ_CAP_BYTES {
        return Err(ReadError::TooLarge(size));
    }

    let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
    if bytes[..sniff_len].contains(&0u8) {
        return Ok(FileDiffBaseResult {
            content: None,
            kind: "binary".to_string(),
        });
    }

    match String::from_utf8(bytes) {
        Ok(content) => Ok(FileDiffBaseResult {
            content: Some(content),
            kind: "text".to_string(),
        }),
        Err(_) => Ok(FileDiffBaseResult {
            content: None,
            kind: "binary".to_string(),
        }),
    }
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

/// Check whether the given byte buffer appears to be binary by sniffing the first
/// [`BINARY_SNIFF_BYTES`] bytes for null bytes.
pub fn is_binary_buffer(bytes: &[u8]) -> bool {
    let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
    bytes[..sniff_len].contains(&0u8)
}

/// Error returned when creating a file or directory.
#[derive(Debug)]
pub enum CreateError {
    AlreadyExists,
    Escape,
    Io(io::Error),
}

impl From<io::Error> for CreateError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::AlreadyExists {
            CreateError::AlreadyExists
        } else {
            CreateError::Io(e)
        }
    }
}

/// Create a new empty file or directory inside `root`.
/// Parent directories are automatically created if they do not exist.
pub fn create_file_or_dir(
    root: &Path,
    rel: &str,
    is_dir: bool,
) -> Result<FileCreateResult, CreateError> {
    let clean_rel = rel.trim_matches('/').replace('\\', "/");
    if clean_rel.is_empty() || clean_rel == "." {
        return Err(CreateError::Escape);
    }
    let Some(path) = worktree_path_allowed(root, &clean_rel) else {
        return Err(CreateError::Escape);
    };
    if path.exists() || std::fs::symlink_metadata(&path).is_ok() {
        return Err(CreateError::AlreadyExists);
    }
    if is_dir {
        std::fs::create_dir_all(&path)?;
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
    }
    Ok(FileCreateResult {
        path: clean_rel,
        is_dir,
    })
}

/// Error returned when renaming a file or directory.
#[derive(Debug)]
pub enum RenameError {
    NotFound,
    AlreadyExists,
    Escape,
    Io(io::Error),
}

impl From<io::Error> for RenameError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => RenameError::NotFound,
            io::ErrorKind::AlreadyExists => RenameError::AlreadyExists,
            _ => RenameError::Io(e),
        }
    }
}

/// Rename or move a path within `root`.
pub fn rename_path(
    root: &Path,
    from_rel: &str,
    to_rel: &str,
) -> Result<FileRenameResult, RenameError> {
    let clean_from = from_rel.trim_matches('/').replace('\\', "/");
    let clean_to = to_rel.trim_matches('/').replace('\\', "/");
    if clean_from.is_empty() || clean_from == "." || clean_to.is_empty() || clean_to == "." {
        return Err(RenameError::Escape);
    }
    let Some(from_path) = worktree_path_allowed(root, &clean_from) else {
        return Err(RenameError::Escape);
    };
    let Some(to_path) = worktree_path_allowed(root, &clean_to) else {
        return Err(RenameError::Escape);
    };
    if !from_path.exists() && std::fs::symlink_metadata(&from_path).is_err() {
        return Err(RenameError::NotFound);
    }
    if to_path.exists() || std::fs::symlink_metadata(&to_path).is_ok() {
        return Err(RenameError::AlreadyExists);
    }
    if let Some(parent) = to_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&from_path, &to_path)?;
    Ok(FileRenameResult {
        from: clean_from,
        to: clean_to,
    })
}

/// Error returned when deleting a file or directory.
#[derive(Debug)]
pub enum DeleteError {
    NotFound,
    Forbidden(String),
    NotEmpty,
    Escape,
    Io(io::Error),
}

impl From<io::Error> for DeleteError {
    fn from(e: io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::NotFound => DeleteError::NotFound,
            io::ErrorKind::DirectoryNotEmpty => DeleteError::NotEmpty,
            _ => DeleteError::Io(e),
        }
    }
}

/// Delete a file or directory within `root`.
pub fn delete_path(
    root: &Path,
    rel: &str,
    recursive: bool,
) -> Result<FileDeleteResult, DeleteError> {
    let clean_rel = rel.trim_matches('/').replace('\\', "/");
    if clean_rel.is_empty() || clean_rel == "." {
        return Err(DeleteError::Forbidden("cannot delete worktree root".into()));
    }
    if clean_rel == ".git" || clean_rel.starts_with(".git/") {
        return Err(DeleteError::Forbidden("cannot delete .git".into()));
    }
    let Some(path) = worktree_path_allowed(root, &clean_rel) else {
        return Err(DeleteError::Escape);
    };
    let root_canon = canonical_prefix(root).unwrap_or_else(|| root.to_path_buf());
    let path_canon = canonical_prefix(&path).unwrap_or_else(|| path.clone());
    if path_canon == root_canon {
        return Err(DeleteError::Forbidden("cannot delete worktree root".into()));
    }
    let meta = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(DeleteError::NotFound),
        Err(e) => return Err(DeleteError::Io(e)),
    };
    if meta.is_dir() {
        if !recursive {
            let mut entries = std::fs::read_dir(&path)?;
            if entries.next().is_some() {
                return Err(DeleteError::NotEmpty);
            }
            std::fs::remove_dir(&path)?;
        } else {
            std::fs::remove_dir_all(&path)?;
        }
    } else {
        std::fs::remove_file(&path)?;
    }
    Ok(FileDeleteResult {
        path: clean_rel,
    })
}

/// Options for searching files in a worktree.
pub struct SearchOpts<'a> {
    pub query: &'a str,
    pub regex: bool,
    pub case_sensitive: bool,
    pub glob: Option<&'a str>,
    pub max_results: usize,
}

/// Error returned when searching worktree content.
#[derive(Debug)]
pub enum SearchError {
    InvalidQuery(String),
    InvalidGlob(String),
    Io(io::Error),
}

/// Search worktree files for a query string or regex pattern.
pub fn search_worktree(
    root: &Path,
    opts: &SearchOpts<'_>,
    file_list: Option<Vec<String>>,
) -> Result<FileSearchResult, SearchError> {
    if opts.query.is_empty() {
        return Ok(FileSearchResult {
            query: String::new(),
            hits: Vec::new(),
            truncated: false,
        });
    }

    let re_pattern = if opts.regex {
        opts.query.to_string()
    } else {
        regex::escape(opts.query)
    };

    let re = regex::RegexBuilder::new(&re_pattern)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .map_err(|e| SearchError::InvalidQuery(e.to_string()))?;

    let glob_matcher = if let Some(g) = opts.glob {
        if !g.trim().is_empty() {
            Some(
                globset::Glob::new(g.trim())
                    .map_err(|e| SearchError::InvalidGlob(e.to_string()))?
                    .compile_matcher(),
            )
        } else {
            None
        }
    } else {
        None
    };

    let files = match file_list {
        Some(f) => f,
        None => index_worktree(root).map_err(SearchError::Io)?.0,
    };

    let cap = opts.max_results.clamp(1, 2000);
    let mut hits = Vec::new();
    let mut truncated = false;

    'search: for rel_path in files {
        if let Some(ref gm) = glob_matcher {
            if !gm.is_match(&rel_path) {
                continue;
            }
        }
        let full_path = root.join(&rel_path);
        let Ok(meta) = std::fs::metadata(&full_path) else {
            continue;
        };
        if meta.is_dir() || meta.len() > READ_CAP_BYTES {
            continue;
        }
        let Ok(bytes) = std::fs::read(&full_path) else {
            continue;
        };
        if is_binary_buffer(&bytes) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };

        for (line_idx, line) in text.lines().enumerate() {
            let line_number = (line_idx + 1) as u32;
            for mat in re.find_iter(line) {
                let column = (line[..mat.start()].chars().count() + 1) as u32;
                if hits.len() >= cap {
                    truncated = true;
                    break 'search;
                }
                hits.push(FileSearchHit {
                    path: rel_path.clone(),
                    line: line_number,
                    column,
                    preview: trim_preview(line, 240),
                });
            }
        }
    }

    Ok(FileSearchResult {
        query: opts.query.to_string(),
        hits,
        truncated,
    })
}

fn trim_preview(line: &str, max_chars: usize) -> String {
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        line.chars().take(max_chars).collect()
    }
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
    fn read_file_detects_binary_and_image_kinds() {
        let dir = tempfile::tempdir().unwrap();

        // Null byte sniffed as binary
        let bin_path = dir.path().join("blob.bin");
        std::fs::write(&bin_path, [b'a', b'b', 0u8, b'c']).unwrap();
        let bin_res = read_file(&bin_path).unwrap();
        assert_eq!(bin_res.kind, "binary");
        assert_eq!(bin_res.content, "");

        // Image extension detected as image
        let img_path = dir.path().join("logo.png");
        std::fs::write(&img_path, [0x89, b'P', b'N', b'G']).unwrap();
        let img_res = read_file(&img_path).unwrap();
        assert_eq!(img_res.kind, "image");
        assert_eq!(img_res.content, "");

        // Text file detected as text
        let txt_path = dir.path().join("hello.txt");
        std::fs::write(&txt_path, "hello text").unwrap();
        let txt_res = read_file(&txt_path).unwrap();
        assert_eq!(txt_res.kind, "text");
        assert_eq!(txt_res.content, "hello text");
    }

    #[test]
    fn read_file_raw_returns_base64_and_mime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pixel.png");
        let png_bytes = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        std::fs::write(&path, png_bytes).unwrap();

        let raw = read_file_raw(&path).unwrap();
        assert_eq!(raw.mime, "image/png");
        assert_eq!(raw.size, png_bytes.len() as u64);
        assert_eq!(
            raw.base64,
            base64::engine::general_purpose::STANDARD.encode(png_bytes)
        );
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

    #[test]
    fn index_worktree_skips_git_and_ignored_directories_and_files() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Initialize git repo so check_ignored works
        let ok = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-b", "main"])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok);

        std::fs::write(root.join(".gitignore"), "target/\nnode_modules/\n*.log\n").unwrap();
        std::fs::write(root.join("README.md"), "# Repo\n").unwrap();

        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("src/nested/deep.txt"), "deep\n").unwrap();

        // Ignored directories and files
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::write(root.join("target/debug/app"), "bin").unwrap();

        std::fs::create_dir_all(root.join("node_modules/foo")).unwrap();
        std::fs::write(root.join("node_modules/foo/index.js"), "console.log()").unwrap();

        std::fs::write(root.join("debug.log"), "log").unwrap();

        let (paths, truncated) = index_worktree(root).unwrap();
        assert!(!truncated);
        assert_eq!(
            paths,
            vec![
                ".gitignore",
                "README.md",
                "src/main.rs",
                "src/nested/deep.txt"
            ]
        );
    }

    #[test]
    fn diff_base_returns_head_version_and_handles_missing_or_binary() {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let git_run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@e.com")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@e.com")
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };

        git_run(&["init", "-b", "main"]);

        std::fs::write(root.join("hello.txt"), "original line\n").unwrap();
        std::fs::write(root.join("binary.dat"), b"hello\0world").unwrap();
        git_run(&["add", "hello.txt", "binary.dat"]);
        git_run(&["commit", "-m", "initial commit"]);

        // 1. diff_base returns committed text content
        let base1 = diff_base(root, "hello.txt").unwrap();
        assert_eq!(base1.kind, "text");
        assert_eq!(base1.content, Some("original line\n".to_string()));

        // 2. Modify hello.txt on disk: diff_base still returns the HEAD version
        std::fs::write(root.join("hello.txt"), "modified line\n").unwrap();
        let base2 = diff_base(root, "hello.txt").unwrap();
        assert_eq!(base2.kind, "text");
        assert_eq!(base2.content, Some("original line\n".to_string()));

        // 3. Binary file returns binary kind with None content
        let base_bin = diff_base(root, "binary.dat").unwrap();
        assert_eq!(base_bin.kind, "binary");
        assert_eq!(base_bin.content, None);

        // 4. Untracked file returns missing kind
        std::fs::write(root.join("new.txt"), "untracked\n").unwrap();
        let base_missing = diff_base(root, "new.txt").unwrap();
        assert_eq!(base_missing.kind, "missing");
        assert_eq!(base_missing.content, None);
    }
}
