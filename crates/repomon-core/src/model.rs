//! The repomon data model.
//!
//! The unit of state is `(repo, worktree)`; a [`Lane`] is the materialized join, optionally
//! carrying live [`AgentSession`]s. All timestamps are stored UTC and converted to local
//! only at render time. Git object ids travel as lowercase hex strings on the wire.

use std::borrow::Cow;
use std::fmt;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub type RepoId = i64;
pub type WorktreeId = i64;
pub type LaneId = i64;
pub type SessionId = i64;

/// (De)serialize a [`gix::ObjectId`] as a lowercase hex string.
pub mod oid_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(oid: &gix::ObjectId, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&oid.to_hex().to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<gix::ObjectId, D::Error> {
        let s = String::deserialize(d)?;
        s.parse::<gix::ObjectId>().map_err(serde::de::Error::custom)
    }
}

/// A registered repository. `path` points at the main worktree, canonical and absolute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub id: RepoId,
    pub path: PathBuf,
    pub name: String,
    pub added_at: DateTime<Utc>,
    pub worktree_root_template: Option<String>,
}

/// A single worktree of a repo (the main checkout is also a worktree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Worktree {
    pub id: WorktreeId,
    pub repo_id: RepoId,
    pub path: PathBuf,
    /// `None` when HEAD is detached.
    pub branch: Option<String>,
    #[serde(with = "oid_hex")]
    pub head: gix::ObjectId,
    pub is_main: bool,
    /// Last path component — what the UI shows.
    pub name: String,
}

/// Counts of staged / unstaged / untracked changes in a worktree.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyState {
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
}

impl DirtyState {
    pub fn is_clean(&self) -> bool {
        self.total() == 0
    }
    pub fn total(&self) -> u32 {
        self.staged + self.unstaged + self.untracked
    }
}

/// The live git state of a worktree — the part that changes as work happens.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeState {
    pub worktree_id: WorktreeId,
    #[serde(with = "oid_hex")]
    pub head: gix::ObjectId,
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: DirtyState,
    pub last_commit_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub prunable: bool,
}

/// A single commit, summarized for timelines and the Today view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    #[serde(with = "oid_hex")]
    pub oid: gix::ObjectId,
    pub repo_id: RepoId,
    pub author_name: String,
    pub author_email: String,
    /// First line of the commit message.
    pub summary: String,
    pub time: DateTime<Utc>,
    pub parent_count: u32,
}

/// The kind of coding agent backing a session. An open enum from day one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentKind {
    ClaudeCode,
    Cursor,
    Aider,
    Codex,
    Other(String),
}

impl AgentKind {
    /// The canonical wire/storage string.
    pub fn as_str(&self) -> Cow<'static, str> {
        match self {
            AgentKind::ClaudeCode => Cow::Borrowed("claude-code"),
            AgentKind::Cursor => Cow::Borrowed("cursor"),
            AgentKind::Aider => Cow::Borrowed("aider"),
            AgentKind::Codex => Cow::Borrowed("codex"),
            AgentKind::Other(s) => Cow::Owned(s.clone()),
        }
    }

    /// The short label shown in the lane row's agent column.
    pub fn short(&self) -> &str {
        match self {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Cursor => "cursor",
            AgentKind::Aider => "aider",
            AgentKind::Codex => "codex",
            AgentKind::Other(s) => s,
        }
    }

    /// The CLI binary to launch for this agent kind.
    pub fn command(&self) -> &str {
        match self {
            AgentKind::ClaudeCode => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Aider => "aider",
            AgentKind::Cursor => "cursor-agent",
            AgentKind::Other(s) => s,
        }
    }

    /// Parse from the wire/storage string (infallible — unknown kinds become `Other`).
    pub fn from_kind_str(s: &str) -> Self {
        match s {
            "claude-code" | "claude" => AgentKind::ClaudeCode,
            "codex" => AgentKind::Codex,
            "aider" => AgentKind::Aider,
            "cursor" => AgentKind::Cursor,
            other => AgentKind::Other(other.to_string()),
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

impl Serialize for AgentKind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_str())
    }
}

impl<'de> Deserialize<'de> for AgentKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(AgentKind::from_kind_str(&s))
    }
}

/// Whether an agent is actively working, waiting on the user, idle, or finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentStatus {
    Running,
    /// Waiting for user input — this is what drives the `⏸` "needs you" flag.
    Waiting,
    #[default]
    Idle,
    Ended,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Waiting => "waiting",
            AgentStatus::Idle => "idle",
            AgentStatus::Ended => "ended",
        }
    }
    /// Does this agent need the user's attention?
    pub fn needs_you(&self) -> bool {
        matches!(self, AgentStatus::Waiting)
    }
}

/// A live (or historical) agent session tied to a `(repo, worktree)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: SessionId,
    pub agent: AgentKind,
    pub repo_id: RepoId,
    pub worktree_id: Option<WorktreeId>,
    pub started_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub manifest_path: PathBuf,
    pub tool_call_count: u32,
    pub title: Option<String>,
    /// Live status, overlaid by the daemon (not persisted).
    #[serde(default)]
    pub status: AgentStatus,
}

/// The materialized `(repo, worktree, agent?)` join — the UI's primary unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lane {
    pub id: LaneId,
    pub repo: Repo,
    pub worktree: Worktree,
    pub state: WorktreeState,
    /// Active sessions only.
    pub agent_sessions: Vec<AgentSession>,
    pub last_activity_at: DateTime<Utc>,
    #[serde(default)]
    pub pinned: bool,
}

/// Persisted per-lane metadata not derivable from git (pin state, tmux window, agent kind).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneMeta {
    pub id: LaneId,
    pub repo_id: RepoId,
    pub worktree_path: PathBuf,
    pub pinned: bool,
    pub tmux_window: Option<String>,
    /// The agent kind repomon last spawned in this lane, if any.
    pub agent_kind: Option<String>,
}

/// Parameters for creating a new lane (and its worktree).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateLaneParams {
    pub repo_id: RepoId,
    pub branch: String,
    #[serde(default)]
    pub source_branch: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub copy_files: Vec<String>,
}

/// A half-open UTC time range `[from, to)`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TimeRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

/// The result of indexing a repo's commits.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncReport {
    pub repo_id: RepoId,
    pub commits_added: u32,
    pub commits_skipped: u32,
    pub errors: Vec<String>,
}

/// Whether a work session was focused on one repo or spanned several in parallel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionKind {
    Focused,
    Parallel,
}

/// A detected window of activity (Phase 3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkSession {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub kind: SessionKind,
    pub repo_ids: Vec<RepoId>,
    pub repo_names: Vec<String>,
    pub commit_count: u32,
}

impl WorkSession {
    pub fn duration_minutes(&self) -> i64 {
        (self.to - self.from).num_minutes()
    }
}

/// One repo's density row in the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineRow {
    pub repo_id: RepoId,
    pub repo_name: String,
    /// Density level (0–5) per time bucket.
    pub density: Vec<u8>,
}

/// A correlation between two repos' active-bucket sets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correlation {
    pub a: String,
    pub b: String,
    pub windows: u32,
    pub overlap: f64,
}

/// The full timeline payload: density rows + correlations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineData {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub bucket_secs: i64,
    pub rows: Vec<TimelineRow>,
    pub correlations: Vec<Correlation>,
}

/// A spawnable agent choice: a built-in kind (detected on PATH) or a configured custom one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChoice {
    /// The name to pass to `agent.spawn` (a kind like "claude-code", or a custom name).
    pub name: String,
    /// The launch command line.
    pub command: String,
    /// Whether the command's binary was found on PATH.
    pub detected: bool,
    /// True if user-defined in config (vs a built-in kind).
    pub custom: bool,
}

/// One entry in the interactive repo browser (directories only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_repo: bool,
    pub added: bool,
}

/// A directory listing for the repo browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseResult {
    pub path: PathBuf,
    pub parent: Option<PathBuf>,
    pub entries: Vec<BrowseEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_kind_roundtrips_through_string() {
        for k in [
            AgentKind::ClaudeCode,
            AgentKind::Codex,
            AgentKind::Aider,
            AgentKind::Cursor,
            AgentKind::Other("amp".into()),
        ] {
            let s = k.as_str().to_string();
            assert_eq!(AgentKind::from_kind_str(&s), k);
        }
        // "claude" is accepted as an alias for claude-code.
        assert_eq!(AgentKind::from_kind_str("claude"), AgentKind::ClaudeCode);
    }

    #[test]
    fn agent_kind_serde_is_a_flat_string() {
        let json = serde_json::to_string(&AgentKind::ClaudeCode).unwrap();
        assert_eq!(json, "\"claude-code\"");
        let back: AgentKind = serde_json::from_str("\"codex\"").unwrap();
        assert_eq!(back, AgentKind::Codex);
    }

    #[test]
    fn dirty_state_cleanliness() {
        assert!(DirtyState::default().is_clean());
        let d = DirtyState {
            staged: 1,
            unstaged: 0,
            untracked: 2,
        };
        assert!(!d.is_clean());
        assert_eq!(d.total(), 3);
    }

    #[test]
    fn oid_serializes_as_hex() {
        let oid: gix::ObjectId = format!("{:040x}", 0xab).parse().unwrap();
        #[derive(Serialize)]
        struct W {
            #[serde(with = "oid_hex")]
            oid: gix::ObjectId,
        }
        let json = serde_json::to_string(&W { oid }).unwrap();
        assert!(json.contains("00000000000000000000000000000000000000ab"));
    }

    #[test]
    fn agent_status_needs_you() {
        assert!(AgentStatus::Waiting.needs_you());
        assert!(!AgentStatus::Running.needs_you());
        assert_eq!(AgentStatus::default(), AgentStatus::Idle);
    }
}
