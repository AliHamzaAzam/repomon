//! Markdown helpers shared by every repomind home writer: frontmatter rendering and parsing,
//! and the slug rule for file names.
//!
//! The home follows the mnemind note conventions (`title`, `type`, `permalink`, `source` in
//! YAML frontmatter, see `~/repomind/AGENTS.md`), so every file the daemon writes there has to
//! render the same shape a hand-written note does, and has to be able to read one back.

/// Render a YAML frontmatter block from ordered key/value pairs, including the `---` fences and
/// the blank line after them.
pub fn frontmatter(fields: &[(&str, String)]) -> String {
    let mut out = String::from("---\n");
    for (key, value) in fields {
        out.push_str(key);
        out.push_str(": ");
        out.push_str(&scalar(value));
        out.push('\n');
    }
    out.push_str("---\n\n");
    out
}

/// Quote a frontmatter value when leaving it bare would produce something YAML reads as a map,
/// a list, or a comment. Titles carry schedule specs like `daily 09:00`, so this matters.
fn scalar(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.trim() != value
        || value.chars().any(|c| {
            matches!(
                c,
                ':' | '#' | '"' | '\'' | '{' | '}' | '[' | ']' | ',' | '\n'
            )
        });
    if !needs_quotes {
        return value.to_string();
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ");
    format!("\"{escaped}\"")
}

/// Split a document into its frontmatter body (without the fences) and the rest. A document with
/// no leading `---` fence yields `(None, whole document)`.
pub fn split_frontmatter(doc: &str) -> (Option<String>, String) {
    let Some(rest) = doc.strip_prefix("---\n") else {
        return (None, doc.to_string());
    };
    let Some(end) = rest.find("\n---\n") else {
        return (None, doc.to_string());
    };
    let block = rest[..end].to_string();
    let body = rest[end + "\n---\n".len()..].trim_start_matches('\n');
    (Some(block), body.to_string())
}

/// Read one scalar field out of a frontmatter block (as produced by [`split_frontmatter`]).
/// Quotes around the value are stripped.
pub fn field(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    frontmatter.lines().find_map(|line| {
        let value = line.strip_prefix(&prefix)?.trim();
        Some(unquote(value))
    })
}

/// Strip one layer of matching quotes, undoing what [`scalar`] added.
fn unquote(value: &str) -> String {
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        return value[1..value.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

/// Reduce free text to a filesystem-safe slug: lowercase, `[a-z0-9-]` only, runs of anything
/// else collapsed to one `-`, trimmed, capped at 48 chars. Empty results fall back to `note`.
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_matches('-').chars().take(48).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("note");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_renders_fenced_yaml_with_a_trailing_blank_line() {
        let out = frontmatter(&[
            ("title", "Journal 2026-09-04".to_string()),
            ("type", "journal".to_string()),
        ]);
        assert_eq!(
            out,
            "---\ntitle: Journal 2026-09-04\ntype: journal\n---\n\n"
        );
    }

    #[test]
    fn frontmatter_quotes_a_value_that_would_break_yaml() {
        // Schedule titles carry the spec, and "daily 09:00" has a colon in it.
        let out = frontmatter(&[("title", "Standing: daily 09:00".to_string())]);
        assert!(
            out.contains("title: \"Standing: daily 09:00\""),
            "got {out:?}"
        );
    }

    #[test]
    fn split_frontmatter_separates_the_block_from_the_body() {
        let doc = "---\ntitle: Notes\nsource: repomond 2026-09-04\n---\n\n# Notes\n\nbody\n";
        let (fm, body) = split_frontmatter(doc);
        assert_eq!(
            fm.as_deref(),
            Some("title: Notes\nsource: repomond 2026-09-04")
        );
        assert_eq!(body, "# Notes\n\nbody\n");
    }

    #[test]
    fn split_frontmatter_returns_the_whole_document_when_there_is_no_block() {
        let (fm, body) = split_frontmatter("# Notes\n\nbody\n");
        assert_eq!(fm, None);
        assert_eq!(body, "# Notes\n\nbody\n");
    }

    #[test]
    fn field_reads_a_scalar_and_strips_quotes() {
        let fm = "title: \"Standing: daily 09:00\"\nstatus: approved";
        assert_eq!(field(fm, "status").as_deref(), Some("approved"));
        assert_eq!(field(fm, "title").as_deref(), Some("Standing: daily 09:00"));
        assert_eq!(field(fm, "missing"), None);
    }

    #[test]
    fn slug_is_lowercase_kebab_and_bounded() {
        assert_eq!(slug("Nightly Fleet Sweep!"), "nightly-fleet-sweep");
        assert_eq!(slug("  --weird--  "), "weird");
        assert_eq!(slug(""), "note");
        assert!(slug(&"x".repeat(200)).len() <= 48);
    }
}
