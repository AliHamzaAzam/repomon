//! Read-only Antigravity conversation monitor.
//!
//! Antigravity 1.1.12 maintains a documented cwd-to-conversation mapping in
//! `last_conversations.json`. Its transcript databases contain protobuf payloads whose status
//! contract is not stable, so repomon uses the mapping only for identity and activity. Managed
//! windows supply the stronger live and attention signals.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{TranscriptSummary, activity_summary};
use crate::model::AgentKind;

pub fn cache_path() -> PathBuf {
    if let Ok(path) = std::env::var("REPOMON_ANTIGRAVITY_CACHE") {
        return PathBuf::from(path);
    }
    directories::BaseDirs::new()
        .map(|dirs| {
            dirs.home_dir()
                .join(".gemini/antigravity-cli/cache/last_conversations.json")
        })
        .unwrap_or_else(|| PathBuf::from(".gemini/antigravity-cli/cache/last_conversations.json"))
}

pub fn summary_for(cwd: &Path) -> Option<TranscriptSummary> {
    let cache = cache_path();
    let mapping: HashMap<String, String> =
        serde_json::from_slice(&std::fs::read(&cache).ok()?).ok()?;
    let wanted = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let (recorded, session_id) = mapping.into_iter().find(|(recorded, _)| {
        let path = PathBuf::from(recorded);
        path.canonicalize().unwrap_or(path) == wanted
    })?;
    let mut summary = activity_summary(AgentKind::Antigravity, &cache)?;
    summary.cwd = Some(PathBuf::from(recorded));
    summary.session_id = Some(session_id);
    Some(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_documented_conversation_mapping() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("last_conversations.json");
        std::fs::write(
            &cache,
            format!(r#"{{"{}":"conversation-1"}}"#, temp.path().display()),
        )
        .unwrap();
        unsafe { std::env::set_var("REPOMON_ANTIGRAVITY_CACHE", &cache) };
        let summary = summary_for(temp.path()).unwrap();
        unsafe { std::env::remove_var("REPOMON_ANTIGRAVITY_CACHE") };
        assert_eq!(summary.kind, AgentKind::Antigravity);
        assert_eq!(summary.session_id.as_deref(), Some("conversation-1"));
    }
}
