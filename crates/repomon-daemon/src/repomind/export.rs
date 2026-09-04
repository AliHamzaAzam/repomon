//! One-way export of the daemon's own records into the repomind home.
//!
//! SQLite stays canonical for everything a machine writes: the orchestration journal, the
//! standing schedules, and the learned approval rules. This module mirrors those rows into
//! markdown so a controller (or a human, or basic-memory) can read them as files:
//!
//! - `journal/YYYY-MM-DD.md`, one section per journal row, keyed by row id in an HTML comment so
//!   a re-run appends only rows the file does not already carry.
//! - `plans/standing/<slug>.md`, one file per schedule.
//! - `profile/approvals.md`, the approval rules grouped by repo.
//!
//! Three rules hold the whole thing together:
//!
//! - **One way.** Nothing here reads a file back into the store. An operator edit to a mirrored
//!   file is overwritten on the next export, which is why every one of them says so in its body.
//! - **Idempotent.** A second run with the same rows writes nothing and commits nothing. Files
//!   are compared before they are written, and the journal is keyed by row id rather than by a
//!   cursor alone, so even a lost state file cannot duplicate a section.
//! - **Bounded blast radius.** Only the three targets above are ever written, and only files the
//!   daemon itself wrote (frontmatter `source: repomond`) are ever removed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use repomon_core::model::{ApprovalRule, JournalEntry, Schedule};
use serde::{Deserialize, Serialize};

use super::md;

/// How long a store write waits for its neighbours before the export runs. Journal rows arrive
/// in bursts (one per MCP action), and a burst should cost one export, not one per row.
pub const DEBOUNCE: Duration = Duration::from_secs(5);

/// The floor between two export commits. Batches keep exporting at the debounce cadence; only
/// the commit is rate limited, so a busy minute lands as one commit carrying every file.
pub const COMMIT_INTERVAL: Duration = Duration::from_secs(60);

/// Longest params/detail digest kept in a journal section. The journal itself is the queryable
/// record; the file is a digest.
const MAX_DIGEST: usize = 300;

/// Most journal rows one export run pulls. A burst is a handful of rows; this only bounds a
/// first run against a long-lived journal, and the next run picks up where it left off.
const JOURNAL_BATCH: usize = 500;

/// The author every export commit carries, so daemon commits are separable from an operator's.
pub const COMMIT_AUTHOR_NAME: &str = "Repomind";
pub const COMMIT_AUTHOR_EMAIL: &str = "repomind@local";

/// Export bookkeeping, persisted at `.repomind/export.json` (daemon-owned and gitignored).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportState {
    /// Highest journal rowid already written to a day file.
    #[serde(default)]
    pub last_journal_id: i64,
    /// When the last export run finished, successfully or not.
    #[serde(default)]
    pub last_run: Option<DateTime<Utc>>,
    /// When the last export commit landed. Drives [`COMMIT_INTERVAL`].
    #[serde(default)]
    pub last_commit_at: Option<DateTime<Utc>>,
    /// The last export failure, cleared by the next successful run.
    #[serde(default)]
    pub last_error: Option<String>,
}

/// The rows one export run mirrors. Read from the store in one go so the run sees a consistent
/// picture and the pure rendering stays testable without a daemon.
#[derive(Debug, Default, Clone)]
pub struct ExportInputs {
    pub journal: Vec<JournalEntry>,
    pub schedules: Vec<Schedule>,
    pub approvals: Vec<ApprovalRule>,
}

/// What one run changed: paths relative to the home (written or removed) and the record kinds
/// they belong to, which become the commit subject.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExportBatch {
    pub touched: Vec<String>,
    pub kinds: Vec<String>,
}

impl ExportBatch {
    pub fn is_empty(&self) -> bool {
        self.touched.is_empty()
    }

    /// Fold another batch in, keeping paths unique and kinds in first-seen order.
    pub fn absorb(&mut self, kind: &str, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        if !self.kinds.iter().any(|k| k == kind) {
            self.kinds.push(kind.to_string());
        }
        for p in paths {
            if !self.touched.contains(&p) {
                self.touched.push(p);
            }
        }
    }
}

/// Work waiting for the next export run. Store writes set `records`; the file-first writers
/// (repo notes, playbooks) add their own paths so those land in the same commit.
#[derive(Debug, Default)]
pub struct Pending {
    /// A journal/schedule/approval write asked for a re-export.
    pub records: bool,
    /// Record kind to home-relative paths written outside the export run itself.
    pub files: BTreeMap<String, BTreeSet<String>>,
}

impl Pending {
    pub fn is_empty(&self) -> bool {
        !self.records && self.files.is_empty()
    }
}

/// What [`commit`] did. Distinguishing "too soon" from "nothing to do" matters: a deferred batch
/// has to be queued again, or the files it exported stay uncommitted forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    /// The batch landed. Carries the commit's short hash.
    Committed(String),
    /// Nothing to commit: an empty batch, or paths git already had.
    Nothing,
    /// The home is not a git repo, so there is no history to write.
    NotAGitRepo,
    /// Inside [`COMMIT_INTERVAL`] since the last commit. The caller queues the batch again.
    TooSoon,
}

/// One export run: what it wrote, and what happened to the commit.
#[derive(Debug, Clone)]
pub struct ExportRun {
    pub batch: ExportBatch,
    pub commit: CommitOutcome,
}

/// `<home>/.repomind/export.json`.
pub fn state_path(home: &Path) -> PathBuf {
    home.join(".repomind").join("export.json")
}

/// Read the export state. A missing or unreadable file is a fresh state, never an error: the
/// journal's row-id markers make a re-export from zero harmless.
pub fn load_state(home: &Path) -> ExportState {
    std::fs::read_to_string(state_path(home))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the export state, creating `.repomind/` if needed.
pub fn save_state(home: &Path, state: &ExportState) -> std::io::Result<()> {
    let path = state_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(path, body + "\n")
}

/// The marker that keys a journal section to its row id. Present in the file, an id is never
/// appended again, which is what makes a re-export (or a lost state file) harmless.
fn row_marker(id: i64) -> String {
    format!("<!-- repomind:row {id} -->")
}

/// Squash a params/detail blob onto one bounded line so a section stays a digest.
fn digest(text: &str) -> String {
    let mut out: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    out = out.replace('`', "'");
    if out.chars().count() > MAX_DIGEST {
        out = out.chars().take(MAX_DIGEST).collect::<String>() + "...";
    }
    out
}

/// One journal row as a keyed markdown section.
fn journal_section(e: &JournalEntry) -> String {
    let at = e.at.with_timezone(&Local);
    let mut out = format!(
        "{}\n## {} {} ({})\n\n",
        row_marker(e.id),
        at.format("%H:%M:%S"),
        e.action,
        e.outcome
    );
    if let Some(repo) = e.repo.as_deref().filter(|r| !r.is_empty()) {
        out.push_str(&format!("- Repo: {repo}\n"));
    }
    if let Some(lane) = e.lane_id {
        out.push_str(&format!("- Lane: {lane}\n"));
    }
    if let Some(params) = e.params.as_deref().filter(|p| !p.trim().is_empty()) {
        out.push_str(&format!("- Params: `{}`\n", digest(params)));
    }
    if let Some(detail) = e.detail.as_deref().filter(|d| !d.trim().is_empty()) {
        out.push_str(&format!("- Detail: {}\n", digest(detail)));
    }
    out.push('\n');
    out
}

/// Write `frontmatter(fields) + body` unless the file already says the same thing. `source`
/// is compared loosely: only its presence matters, so a re-export on a later day does not
/// rewrite (and re-commit) a file whose content is unchanged. Returns whether it wrote.
fn write_if_changed(path: &Path, fields: &[(&str, String)], body: &str) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let (fm, existing_body) = md::split_frontmatter(&existing);
        let same_fields = fm.as_deref().is_some_and(|fm| {
            fields
                .iter()
                .all(|(k, v)| *k == "source" || md::field(fm, k).as_deref() == Some(v.as_str()))
        });
        if same_fields && existing_body == body {
            return Ok(false);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, md::frontmatter(fields) + body)?;
    Ok(true)
}

pub fn export_journal(home: &Path, entries: &[JournalEntry]) -> std::io::Result<Vec<String>> {
    // Group by the operator's local date: a day file is a day as they lived it, not as UTC did.
    let mut by_day: BTreeMap<String, Vec<&JournalEntry>> = BTreeMap::new();
    for e in entries {
        let day = e.at.with_timezone(&Local).format("%Y-%m-%d").to_string();
        by_day.entry(day).or_default().push(e);
    }

    let mut touched = Vec::new();
    for (day, mut rows) in by_day {
        rows.sort_by_key(|e| e.id);
        let rel = format!("journal/{day}.md");
        let path = home.join(&rel);
        let mut doc = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                md::frontmatter(&[
                    ("title", format!("Journal {day}")),
                    ("type", "journal".to_string()),
                    ("permalink", format!("repomind/journal/{day}")),
                    ("source", format!("repomond {day}")),
                ]) + &format!(
                    "# Journal {day}\n\n\
                     Exported one way from the daemon's orchestration journal, one section per\n\
                     action. Read it; do not hand-edit it.\n\n"
                )
            }
            Err(e) => return Err(e),
        };

        let mut appended = false;
        for e in rows {
            if doc.contains(&row_marker(e.id)) {
                continue;
            }
            doc.push_str(&journal_section(e));
            appended = true;
        }
        if !appended {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, doc)?;
        touched.push(rel);
    }
    Ok(touched)
}

/// The mirror file name for a schedule, collision-aware against the other schedules the same
/// way `repomon_core::notes` handles two repos with one name.
fn schedule_slug(s: &Schedule, all: &[Schedule]) -> String {
    let base = md::slug(&s.prompt);
    let collides = all
        .iter()
        .any(|other| other.id != s.id && md::slug(&other.prompt) == base);
    if collides {
        format!("{base}-{}", s.id)
    } else {
        base
    }
}

fn schedule_body(s: &Schedule) -> String {
    let created = s.created_at.with_timezone(&Local);
    let last = s
        .last_run_at
        .map(|t| {
            t.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "never".to_string());
    // Anchored on the last run (or creation), never on "now": a mirror whose next-run line moved
    // every few minutes would rewrite and re-commit the file on every export.
    let next = repomon_core::schedule::parse_spec(&s.spec)
        .map(|spec| {
            spec.next_after(s.last_run_at.unwrap_or(s.created_at).with_timezone(&Local))
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| "unparseable spec".to_string());
    format!(
        "# Standing: {spec}\n\n\
         - Spec: {spec}\n\
         - Action cap: {cap}\n\
         - Created: {created}\n\
         - Last run: {last}\n\
         - Next run: {next}\n\n\
         ## Goal\n\n\
         {prompt}\n\n\
         Mirrored from the daemon's schedule {id}. The daemon is the source of truth: change the\n\
         schedule itself, not this file.\n",
        spec = s.spec,
        cap = s.max_actions,
        created = created.format("%Y-%m-%d"),
        prompt = s.prompt.trim(),
        id = s.id,
    )
}

pub fn export_schedules(home: &Path, schedules: &[Schedule]) -> std::io::Result<Vec<String>> {
    let dir = home.join("plans").join("standing");
    std::fs::create_dir_all(&dir)?;
    let mut touched = Vec::new();
    let mut ours: BTreeSet<String> = BTreeSet::new();

    for s in schedules {
        let name = format!("{}.md", schedule_slug(s, schedules));
        ours.insert(name.clone());
        let fields = [
            ("title", format!("Standing: {}", s.spec)),
            ("type", "plan".to_string()),
            (
                "permalink",
                format!("repomind/plans/standing/{}", schedule_slug(s, schedules)),
            ),
            ("source", format!("repomond {}", Utc::now().format("%Y-%m-%d"))),
            ("status", "active".to_string()),
            ("schedule", s.id.to_string()),
        ];
        if write_if_changed(&dir.join(&name), &fields, &schedule_body(s))? {
            touched.push(format!("plans/standing/{name}"));
        }
    }

    // Retire mirrors of schedules that are gone. Only files carrying a `schedule:` id in their
    // frontmatter are ours; a hand-written plan in the same directory is never touched.
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") || ours.contains(&name) {
            continue;
        }
        let Ok(doc) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let is_mirror = md::split_frontmatter(&doc)
            .0
            .is_some_and(|fm| md::field(&fm, "schedule").is_some());
        if is_mirror {
            std::fs::remove_file(entry.path())?;
            touched.push(format!("plans/standing/{name}"));
        }
    }
    touched.sort();
    Ok(touched)
}

pub fn export_approvals(home: &Path, rules: &[ApprovalRule]) -> std::io::Result<Vec<String>> {
    let mut by_repo: BTreeMap<&str, Vec<&ApprovalRule>> = BTreeMap::new();
    for r in rules {
        by_repo.entry(r.repo.as_str()).or_default().push(r);
    }

    let mut body = String::from(
        "# Approval rules\n\n\
         Bash permission patterns a human has confirmed per repo; the daemon auto-approves a\n\
         matching dialog without asking again. Exported one way from the daemon store, so an\n\
         edit here changes nothing: add or remove rules through Repomon.\n\n",
    );
    if by_repo.is_empty() {
        body.push_str("No approval rules have been confirmed yet.\n");
    }
    for (repo, mut rules) in by_repo {
        rules.sort_by(|a, b| a.pattern.cmp(&b.pattern));
        body.push_str(&format!("## {repo}\n\n"));
        for r in rules {
            body.push_str(&format!(
                "- `{}` (confirmed {})\n",
                r.pattern,
                r.created_at.with_timezone(&Local).format("%Y-%m-%d")
            ));
        }
        body.push('\n');
    }

    let fields = [
        ("title", "Approval rules".to_string()),
        ("type", "note".to_string()),
        ("permalink", "repomind/profile/approvals".to_string()),
        ("source", format!("repomond {}", Utc::now().format("%Y-%m-%d"))),
    ];
    // With no rules and no file yet there is nothing to say: an empty home should not sprout a
    // "no approval rules" note it never asked for. Once the file exists (the operator's template
    // note, or an earlier export), it is kept current even when the last rule is removed.
    let path = home.join("profile").join("approvals.md");
    if rules.is_empty() && !path.exists() {
        return Ok(Vec::new());
    }
    if write_if_changed(&path, &fields, &body)? {
        Ok(vec!["profile/approvals.md".to_string()])
    } else {
        Ok(Vec::new())
    }
}

pub fn export_all(
    home: &Path,
    inputs: &ExportInputs,
    state: &mut ExportState,
) -> std::io::Result<ExportBatch> {
    let mut batch = ExportBatch::default();
    batch.absorb("journal", export_journal(home, &inputs.journal)?);
    batch.absorb("schedules", export_schedules(home, &inputs.schedules)?);
    batch.absorb("approvals", export_approvals(home, &inputs.approvals)?);
    if let Some(highest) = inputs.journal.iter().map(|e| e.id).max() {
        state.last_journal_id = state.last_journal_id.max(highest);
    }
    Ok(batch)
}

/// Run one git command in the home, returning stdout on success.
fn git(home: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("git")
        .args(args)
        .current_dir(home)
        .output()
}

pub fn commit(
    home: &Path,
    batch: &ExportBatch,
    now: DateTime<Utc>,
    state: &mut ExportState,
) -> std::io::Result<CommitOutcome> {
    if batch.is_empty() {
        return Ok(CommitOutcome::Nothing);
    }
    if !home.join(".git").exists() {
        return Ok(CommitOutcome::NotAGitRepo);
    }
    if let Some(last) = state.last_commit_at {
        if now.signed_duration_since(last).to_std().unwrap_or_default() < COMMIT_INTERVAL {
            return Ok(CommitOutcome::TooSoon);
        }
    }

    // Stage only the batch's own paths. `git add` on a removed path stages the deletion, and an
    // operator's untracked file elsewhere in the home is never named, so it stays untracked.
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(batch.touched.iter().map(String::as_str));
    let add = git(home, &args)?;
    if !add.status.success() {
        return Err(std::io::Error::other(format!(
            "git add in {} failed: {}",
            home.display(),
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }

    let staged = git(home, &["diff", "--cached", "--name-only"])?;
    if String::from_utf8_lossy(&staged.stdout).trim().is_empty() {
        return Ok(CommitOutcome::Nothing);
    }

    let subject = format!("chore(repomind): export {}", batch.kinds.join(", "));
    let author = format!("{COMMIT_AUTHOR_NAME} <{COMMIT_AUTHOR_EMAIL}>");
    let name_cfg = format!("user.name={COMMIT_AUTHOR_NAME}");
    let email_cfg = format!("user.email={COMMIT_AUTHOR_EMAIL}");
    // `--only <paths>` commits exactly the batch, so anything an operator happened to stage in
    // the home while the export ran stays staged rather than riding along in a daemon commit.
    let mut args: Vec<&str> = vec![
        "-c",
        &name_cfg,
        "-c",
        &email_cfg,
        "commit",
        "--only",
        "--no-verify",
        "--author",
        &author,
        "-m",
        &subject,
        "--",
    ];
    args.extend(batch.touched.iter().map(String::as_str));
    let out = git(home, &args)?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "git commit in {} failed: {}",
            home.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }

    state.last_commit_at = Some(now);
    let hash = git(home, &["rev-parse", "--short", "HEAD"])?;
    Ok(CommitOutcome::Committed(
        String::from_utf8_lossy(&hash.stdout).trim().to_string(),
    ))
}

// ---- daemon driver ---------------------------------------------------------

/// Ask for an export because a journal, schedule, or approval-rule row changed. Debounced.
pub async fn request(ctx: &crate::Ctx) {
    ctx.repomind_export.lock().await.records = true;
    ctx.repomind_export_wake.notify_one();
}

/// Ask for an export and carry `paths` (home-relative, already written by the caller) into the
/// next commit under `kind`. Used by the file-first writers, which own their own files.
pub async fn request_files(ctx: &crate::Ctx, kind: &str, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    {
        let mut pending = ctx.repomind_export.lock().await;
        pending
            .files
            .entry(kind.to_string())
            .or_default()
            .extend(paths);
    }
    ctx.repomind_export_wake.notify_one();
}

/// Whether anything is waiting for the next export run.
pub async fn pending(ctx: &crate::Ctx) -> bool {
    !ctx.repomind_export.lock().await.is_empty()
}

/// The debounced export loop: wake on the first request, wait [`DEBOUNCE`] for its neighbours,
/// then run once for the whole burst.
pub async fn export_watch(ctx: std::sync::Arc<crate::Ctx>) {
    loop {
        ctx.repomind_export_wake.notified().await;
        loop {
            tokio::time::sleep(DEBOUNCE).await;
            match run_now(&ctx).await {
                // The batch was exported but its commit is inside the once-a-minute floor.
                // `run_now` queued it again; wait the floor out rather than spinning on it.
                Ok(run) if run.commit == CommitOutcome::TooSoon => {
                    tokio::time::sleep(COMMIT_INTERVAL).await;
                }
                Ok(_) => break,
                Err(e) => {
                    tracing::warn!("repomind export failed: {e}");
                    break;
                }
            }
        }
    }
}

/// Run one export now: read the records, render the files, commit the batch. Skips silently
/// when the home does not exist yet (ensure-home has not run, or the operator moved it).
pub async fn run_now(ctx: &crate::Ctx) -> repomon_core::Result<ExportRun> {
    let _guard = ctx.repomind_export_lock.lock().await;
    let home = ctx.config.read().await.repomind_home();

    // Drain first: anything a writer adds after this point wakes the loop again rather than
    // being dropped by a run that had already rendered its files.
    let drained = std::mem::take(&mut *ctx.repomind_export.lock().await);

    {
        let probe = home.clone();
        let exists = tokio::task::spawn_blocking(move || probe.is_dir())
            .await
            .map_err(|e| repomon_core::Error::Other(e.to_string()))?;
        if !exists {
            return Ok(ExportRun {
                batch: ExportBatch::default(),
                commit: CommitOutcome::Nothing,
            });
        }
    }

    let state = {
        let probe = home.clone();
        tokio::task::spawn_blocking(move || load_state(&probe))
            .await
            .map_err(|e| repomon_core::Error::Other(e.to_string()))?
    };

    // Only rows the cursor has not seen: the day files key on row id anyway, but reading the
    // whole journal on every burst would grow linearly with the fleet's lifetime.
    let inputs = ExportInputs {
        journal: ctx
            .store
            .journal_after(state.last_journal_id, JOURNAL_BATCH)
            .await?,
        schedules: ctx.store.list_schedules().await?,
        approvals: ctx.store.list_approval_rules().await?,
    };

    let now = chrono::Utc::now();
    let outcome = tokio::task::spawn_blocking(move || -> std::io::Result<ExportRun> {
        let mut state = state;
        let mut batch = export_all(&home, &inputs, &mut state)?;
        for (kind, paths) in drained.files {
            batch.absorb(&kind, paths.into_iter().collect());
        }
        let result = commit(&home, &batch, now, &mut state);
        state.last_run = Some(now);
        state.last_error = result.as_ref().err().map(ToString::to_string);
        save_state(&home, &state)?;
        Ok(ExportRun {
            batch,
            commit: result?,
        })
    })
    .await
    .map_err(|e| repomon_core::Error::Other(e.to_string()))?;

    let run = outcome.map_err(repomon_core::Error::Io)?;
    if !run.batch.is_empty() {
        tracing::info!(
            files = run.batch.touched.len(),
            kinds = %run.batch.kinds.join(", "),
            committed = matches!(run.commit, CommitOutcome::Committed(_)),
            "repomind export"
        );
    }
    // A commit held back by the once-a-minute floor would otherwise strand its files: the batch
    // has already been drained, and re-exporting finds nothing to do because the content is
    // written. Queue the same paths again so the next run stages and commits them.
    if run.commit == CommitOutcome::TooSoon {
        let mut queued = ctx.repomind_export.lock().await;
        for kind in &run.batch.kinds {
            queued
                .files
                .entry(kind.clone())
                .or_default()
                .extend(run.batch.touched.iter().cloned());
        }
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        super::super::ensure_layout(&home).unwrap();
        (dir, home)
    }

    fn row(id: i64, at: DateTime<Utc>, action: &str) -> JournalEntry {
        JournalEntry {
            id,
            at,
            session: "s1".into(),
            action: action.into(),
            lane_id: Some(7),
            repo: Some("repomon".into()),
            params: Some("{\"agent\":\"claude\"}".into()),
            outcome: "ok".into(),
            detail: Some("spawned lane-7-1".into()),
        }
    }

    /// Local noon on the given day, so a day file's name cannot flip with the test machine's
    /// timezone the way a UTC midnight timestamp would.
    fn local_noon(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn journal_export_writes_a_day_file_with_frontmatter_and_one_section_per_row() {
        let (_d, home) = home();
        let paths = export_journal(
            &home,
            &[
                row(1, local_noon(2026, 9, 4), "spawn_agent"),
                row(2, local_noon(2026, 9, 4), "merge_lane"),
            ],
        )
        .unwrap();

        assert_eq!(paths, vec!["journal/2026-09-04.md".to_string()]);
        let body = std::fs::read_to_string(home.join("journal/2026-09-04.md")).unwrap();
        let (fm, _) = md::split_frontmatter(&body);
        let fm = fm.expect("day file needs frontmatter");
        assert_eq!(md::field(&fm, "type").as_deref(), Some("journal"));
        assert_eq!(
            md::field(&fm, "permalink").as_deref(),
            Some("repomind/journal/2026-09-04")
        );
        assert!(md::field(&fm, "source").unwrap().starts_with("repomond"));
        assert!(body.contains("<!-- repomind:row 1 -->"), "{body}");
        assert!(body.contains("<!-- repomind:row 2 -->"), "{body}");
        assert!(body.contains("spawn_agent"), "{body}");
    }

    #[test]
    fn journal_export_appends_only_rows_the_day_file_does_not_carry() {
        let (_d, home) = home();
        let day = local_noon(2026, 9, 4);
        export_journal(&home, &[row(1, day, "spawn_agent")]).unwrap();

        let paths = export_journal(
            &home,
            &[row(1, day, "spawn_agent"), row(2, day, "merge_lane")],
        )
        .unwrap();

        assert_eq!(paths, vec!["journal/2026-09-04.md".to_string()]);
        let body = std::fs::read_to_string(home.join("journal/2026-09-04.md")).unwrap();
        assert_eq!(body.matches("<!-- repomind:row 1 -->").count(), 1, "{body}");
        assert_eq!(body.matches("<!-- repomind:row 2 -->").count(), 1, "{body}");
    }

    #[test]
    fn journal_export_is_idempotent() {
        let (_d, home) = home();
        let rows = [row(1, local_noon(2026, 9, 4), "spawn_agent")];
        export_journal(&home, &rows).unwrap();
        let before = std::fs::read_to_string(home.join("journal/2026-09-04.md")).unwrap();

        let paths = export_journal(&home, &rows).unwrap();

        assert!(paths.is_empty(), "a re-run must touch nothing, got {paths:?}");
        assert_eq!(
            std::fs::read_to_string(home.join("journal/2026-09-04.md")).unwrap(),
            before
        );
    }

    #[test]
    fn journal_export_splits_rows_across_day_files_by_local_date() {
        let (_d, home) = home();
        export_journal(
            &home,
            &[
                row(1, local_noon(2026, 9, 3), "spawn_agent"),
                row(2, local_noon(2026, 9, 4), "merge_lane"),
            ],
        )
        .unwrap();

        assert!(home.join("journal/2026-09-03.md").is_file());
        assert!(home.join("journal/2026-09-04.md").is_file());
    }

    fn schedule(id: i64, spec: &str, prompt: &str) -> Schedule {
        Schedule {
            id,
            spec: spec.into(),
            prompt: prompt.into(),
            max_actions: 10,
            created_at: local_noon(2026, 9, 1),
            last_run_at: None,
        }
    }

    #[test]
    fn schedule_export_writes_one_plan_file_per_schedule() {
        let (_d, home) = home();
        let paths =
            export_schedules(&home, &[schedule(1, "daily 09:00", "Sweep the fleet for stalls")])
                .unwrap();

        assert_eq!(
            paths,
            vec!["plans/standing/sweep-the-fleet-for-stalls.md".to_string()]
        );
        let body =
            std::fs::read_to_string(home.join("plans/standing/sweep-the-fleet-for-stalls.md"))
                .unwrap();
        let (fm, _) = md::split_frontmatter(&body);
        let fm = fm.expect("standing plan needs frontmatter");
        assert_eq!(md::field(&fm, "schedule").as_deref(), Some("1"));
        assert_eq!(md::field(&fm, "type").as_deref(), Some("plan"));
        assert!(body.contains("daily 09:00"), "{body}");
        assert!(body.contains("Sweep the fleet for stalls"), "{body}");
        assert!(body.contains("10"), "the action cap belongs in the file: {body}");
    }

    #[test]
    fn schedule_export_is_idempotent_and_reflects_a_removal() {
        let (_d, home) = home();
        let scheds = [schedule(1, "daily 09:00", "Sweep the fleet")];
        export_schedules(&home, &scheds).unwrap();
        assert!(
            export_schedules(&home, &scheds).unwrap().is_empty(),
            "a re-run must touch nothing"
        );

        let paths = export_schedules(&home, &[]).unwrap();

        assert_eq!(paths, vec!["plans/standing/sweep-the-fleet.md".to_string()]);
        assert!(!home.join("plans/standing/sweep-the-fleet.md").exists());
    }

    #[test]
    fn schedule_export_never_removes_a_hand_written_plan() {
        let (_d, home) = home();
        let hand = home.join("plans/standing/README.md");
        std::fs::write(&hand, "---\ntitle: Standing plans\n---\n\nby hand\n").unwrap();

        export_schedules(&home, &[]).unwrap();

        assert_eq!(
            std::fs::read_to_string(&hand).unwrap(),
            "---\ntitle: Standing plans\n---\n\nby hand\n"
        );
    }

    fn rule(repo: &str, pattern: &str) -> ApprovalRule {
        ApprovalRule {
            repo: repo.into(),
            pattern: pattern.into(),
            created_at: local_noon(2026, 9, 1),
        }
    }

    #[test]
    fn approval_export_groups_the_rules_by_repo() {
        let (_d, home) = home();
        let paths = export_approvals(
            &home,
            &[
                rule("repomon", "cargo test"),
                rule("repomon", "git status"),
                rule("mira", "bun test"),
            ],
        )
        .unwrap();

        assert_eq!(paths, vec!["profile/approvals.md".to_string()]);
        let body = std::fs::read_to_string(home.join("profile/approvals.md")).unwrap();
        assert!(body.contains("## mira"), "{body}");
        assert!(body.contains("## repomon"), "{body}");
        assert!(body.contains("cargo test"), "{body}");
        assert!(body.contains("bun test"), "{body}");
        // One heading per repo, not one per rule.
        assert_eq!(body.matches("## repomon").count(), 1, "{body}");
    }

    #[test]
    fn approval_export_is_idempotent() {
        let (_d, home) = home();
        let rules = [rule("repomon", "cargo test")];
        export_approvals(&home, &rules).unwrap();
        assert!(export_approvals(&home, &rules).unwrap().is_empty());
    }

    #[test]
    fn export_all_advances_the_journal_cursor_and_names_every_kind() {
        let (_d, home) = home();
        let inputs = ExportInputs {
            journal: vec![row(4, local_noon(2026, 9, 4), "spawn_agent")],
            schedules: vec![schedule(1, "daily 09:00", "Sweep the fleet")],
            approvals: vec![rule("repomon", "cargo test")],
        };
        let mut state = ExportState::default();

        let batch = export_all(&home, &inputs, &mut state).unwrap();

        assert_eq!(state.last_journal_id, 4);
        assert_eq!(batch.kinds, vec!["journal", "schedules", "approvals"]);
        assert_eq!(batch.touched.len(), 3, "{batch:?}");

        let again = export_all(&home, &inputs, &mut state).unwrap();
        assert!(again.is_empty(), "a re-run must be a no-op, got {again:?}");
    }

    #[test]
    fn export_all_writes_nothing_outside_its_three_targets() {
        let (_d, home) = home();
        let inputs = ExportInputs {
            journal: vec![row(1, local_noon(2026, 9, 4), "spawn_agent")],
            schedules: vec![schedule(1, "daily 09:00", "Sweep the fleet")],
            approvals: vec![rule("repomon", "cargo test")],
        };
        let mut state = ExportState::default();

        let batch = export_all(&home, &inputs, &mut state).unwrap();

        for path in &batch.touched {
            assert!(
                path.starts_with("journal/")
                    || path.starts_with("plans/standing/")
                    || path == "profile/approvals.md",
                "{path} is outside the export targets"
            );
        }
    }

    #[test]
    fn state_round_trips_through_the_scratch_dir() {
        let (_d, home) = home();
        assert_eq!(load_state(&home), ExportState::default());
        let state = ExportState {
            last_journal_id: 12,
            last_run: Some(local_noon(2026, 9, 4)),
            last_commit_at: None,
            last_error: Some("disk full".into()),
        };
        save_state(&home, &state).unwrap();
        assert_eq!(load_state(&home), state);
    }

    // ---- commits (brief item 4) ---------------------------------------------

    fn git(home: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(home)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn git_home() -> (tempfile::TempDir, PathBuf) {
        let (d, home) = home();
        super::super::ensure_git_repo(&home).unwrap();
        (d, home)
    }

    #[test]
    fn commit_makes_one_commit_authored_by_repomind() {
        let (_d, home) = git_home();
        let mut state = ExportState::default();
        let batch = export_all(
            &home,
            &ExportInputs {
                journal: vec![row(1, local_noon(2026, 9, 4), "spawn_agent")],
                ..Default::default()
            },
            &mut state,
        )
        .unwrap();

        let outcome = commit(&home, &batch, Utc::now(), &mut state).unwrap();

        assert!(
            matches!(outcome, CommitOutcome::Committed(_)),
            "the batch should have produced a commit, got {outcome:?}"
        );
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(
            git(&home, &["log", "-1", "--format=%an <%ae>"]),
            format!("{COMMIT_AUTHOR_NAME} <{COMMIT_AUTHOR_EMAIL}>")
        );
        assert_eq!(
            git(&home, &["log", "-1", "--format=%s"]),
            "chore(repomind): export journal"
        );
        assert!(state.last_commit_at.is_some());
    }

    #[test]
    fn commit_batches_at_most_once_per_minute() {
        let (_d, home) = git_home();
        let mut state = ExportState::default();
        let now = Utc::now();
        let first = export_all(
            &home,
            &ExportInputs {
                journal: vec![row(1, local_noon(2026, 9, 4), "spawn_agent")],
                ..Default::default()
            },
            &mut state,
        )
        .unwrap();
        commit(&home, &first, now, &mut state).unwrap();

        let second = export_all(
            &home,
            &ExportInputs {
                journal: vec![
                    row(1, local_noon(2026, 9, 4), "spawn_agent"),
                    row(2, local_noon(2026, 9, 4), "merge_lane"),
                ],
                ..Default::default()
            },
            &mut state,
        )
        .unwrap();
        let too_soon = commit(&home, &second, now + chrono::Duration::seconds(5), &mut state)
            .unwrap();
        assert_eq!(
            too_soon,
            CommitOutcome::TooSoon,
            "a second commit within a minute must wait"
        );
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");

        let later = commit(&home, &second, now + chrono::Duration::seconds(90), &mut state)
            .unwrap();
        assert!(matches!(later, CommitOutcome::Committed(_)), "{later:?}");
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "2");
    }

    #[test]
    fn commit_leaves_untracked_operator_files_alone() {
        let (_d, home) = git_home();
        std::fs::write(home.join("knowledge/draft.md"), "operator scratch\n").unwrap();
        let mut state = ExportState::default();
        let batch = export_all(
            &home,
            &ExportInputs {
                journal: vec![row(1, local_noon(2026, 9, 4), "spawn_agent")],
                ..Default::default()
            },
            &mut state,
        )
        .unwrap();

        commit(&home, &batch, Utc::now(), &mut state).unwrap();

        assert!(
            git(&home, &["status", "--porcelain", "--untracked-files=all"])
                .contains("?? knowledge/draft.md"),
            "the operator's scratch file must still be untracked"
        );
        assert_eq!(
            git(&home, &["log", "-1", "--name-only", "--format="]),
            "journal/2026-09-04.md"
        );
    }

    #[test]
    fn commit_is_silently_skipped_when_the_home_is_not_a_git_repo() {
        let (_d, home) = home();
        let mut state = ExportState::default();
        let batch = export_all(
            &home,
            &ExportInputs {
                journal: vec![row(1, local_noon(2026, 9, 4), "spawn_agent")],
                ..Default::default()
            },
            &mut state,
        )
        .unwrap();

        assert_eq!(
            commit(&home, &batch, Utc::now(), &mut state).unwrap(),
            CommitOutcome::NotAGitRepo
        );
    }

    #[test]
    fn commit_of_an_empty_batch_does_nothing() {
        let (_d, home) = git_home();
        let mut state = ExportState::default();
        assert_eq!(
            commit(&home, &ExportBatch::default(), Utc::now(), &mut state).unwrap(),
            CommitOutcome::Nothing
        );
        assert!(state.last_commit_at.is_none());
    }

    // ---- daemon driver ------------------------------------------------------

    async fn test_ctx(home: &Path) -> std::sync::Arc<crate::Ctx> {
        let store = repomon_core::Store::open_in_memory().unwrap();
        let mut config = repomon_core::Config::default();
        config.repomind.home = home.to_string_lossy().into_owned();
        crate::Ctx::new(store, config, None)
    }

    #[tokio::test]
    async fn run_now_exports_the_store_and_commits_in_the_home() {
        let (_d, home) = git_home();
        let ctx = test_ctx(&home).await;
        ctx.store
            .append_journal(row(0, Utc::now(), "spawn_agent"))
            .await
            .unwrap();

        let run = run_now(&ctx).await.unwrap();

        assert_eq!(run.batch.kinds, vec!["journal"]);
        let day = Local::now().format("%Y-%m-%d").to_string();
        assert!(home.join(format!("journal/{day}.md")).is_file());
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");
        assert_eq!(
            git(&home, &["log", "-1", "--format=%an"]),
            COMMIT_AUTHOR_NAME
        );
        // The cursor and the run stamp survive for `repomind.status`.
        let state = load_state(&home);
        assert!(state.last_journal_id > 0);
        assert!(state.last_run.is_some());
        assert_eq!(state.last_error, None);
    }

    #[tokio::test]
    async fn run_now_is_a_no_op_the_second_time() {
        let (_d, home) = git_home();
        let ctx = test_ctx(&home).await;
        ctx.store
            .append_journal(row(0, Utc::now(), "spawn_agent"))
            .await
            .unwrap();
        run_now(&ctx).await.unwrap();

        let again = run_now(&ctx).await.unwrap();

        assert!(again.batch.is_empty(), "got {again:?}");
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");
    }

    #[tokio::test]
    async fn run_now_skips_a_home_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("never-created");
        let ctx = test_ctx(&home).await;

        let run = run_now(&ctx).await.unwrap();

        assert!(run.batch.is_empty());
        assert!(!home.exists(), "run_now must not create the home itself");
    }

    #[tokio::test]
    async fn run_now_commits_the_paths_a_file_first_writer_left_pending() {
        let (_d, home) = git_home();
        let ctx = test_ctx(&home).await;
        std::fs::create_dir_all(home.join("fleet/repomon")).unwrap();
        std::fs::write(home.join("fleet/repomon/notes.md"), "notes
").unwrap();
        request_files(&ctx, "notes", vec!["fleet/repomon/notes.md".to_string()]).await;
        assert!(pending(&ctx).await);

        let run = run_now(&ctx).await.unwrap();

        assert_eq!(run.batch.kinds, vec!["notes"]);
        assert_eq!(
            git(&home, &["log", "-1", "--format=%s"]),
            "chore(repomind): export notes"
        );
        assert!(!pending(&ctx).await, "the run must drain what it exported");
    }

    #[tokio::test]
    async fn request_marks_the_export_pending_until_a_run_drains_it() {
        let (_d, home) = git_home();
        let ctx = test_ctx(&home).await;
        assert!(!pending(&ctx).await);

        request(&ctx).await;

        assert!(pending(&ctx).await);
        run_now(&ctx).await.unwrap();
        assert!(!pending(&ctx).await);
    }

    /// A batch whose commit is inside the once-a-minute floor must not be lost: it is queued
    /// again so the next run commits it. Before this, a file exported 30 s after a commit stayed
    /// uncommitted forever, because the batch that carried it had already been drained.
    #[tokio::test]
    async fn a_rate_limited_batch_is_requeued_and_commits_on_the_next_run() {
        let (_d, home) = git_home();
        let ctx = test_ctx(&home).await;
        ctx.store
            .append_journal(row(0, Utc::now(), "spawn_agent"))
            .await
            .unwrap();
        run_now(&ctx).await.unwrap();
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");

        ctx.store
            .append_journal(row(0, Utc::now(), "merge_lane"))
            .await
            .unwrap();
        let run = run_now(&ctx).await.unwrap();

        assert!(matches!(run.commit, CommitOutcome::TooSoon), "{:?}", run.commit);
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "1");
        assert!(
            pending(&ctx).await,
            "a deferred commit must leave its batch queued"
        );

        // Age the commit stamp past the floor, the way a minute of wall clock would.
        let mut state = load_state(&home);
        state.last_commit_at = Some(Utc::now() - chrono::Duration::minutes(2));
        save_state(&home, &state).unwrap();

        let run = run_now(&ctx).await.unwrap();

        assert!(matches!(run.commit, CommitOutcome::Committed(_)), "{:?}", run.commit);
        assert_eq!(git(&home, &["rev-list", "--count", "HEAD"]), "2");
        assert!(
            git(&home, &["log", "-1", "--name-only", "--format="]).contains("journal/"),
            "the deferred day file must be in the new commit"
        );
        assert!(!pending(&ctx).await);
    }
}
