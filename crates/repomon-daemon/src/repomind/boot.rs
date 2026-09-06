//! The boot context: one bounded markdown document a controller reads before it does anything.
//!
//! A fresh controller starts with no memory of the fleet. Rather than make it call tools to find
//! out what is going on, the daemon assembles the home's standing knowledge into
//! `.repomind/boot.md` and hands it to the agent at spawn (see `rpc.rs` for the per-backend
//! delivery). The document is daemon-owned: rewritten on every spawn, gitignored by R1's
//! `.gitignore`, and never hand-edited.
//!
//! Two properties make it safe to hand to any backend:
//!
//! - **Bounded.** A hard token budget ([`DEFAULT_BUDGET_TOKENS`]) with a coarse
//!   four-characters-per-token estimate. Over budget, whole files are dropped from the least
//!   important end (journal, then profile, then plans) and the document ends by naming them, so
//!   a controller can tell the difference between "nothing to say" and "it did not fit".
//! - **Pure.** [`assemble_boot`] reads the home and nothing else, so every ordering and trimming
//!   rule is testable against a tempdir home without a daemon.

use std::path::{Path, PathBuf};

use chrono::{Duration, Local};

use super::md;

/// The boot budget, in tokens, when `[repomind] boot_budget_tokens` is left at its default.
pub const DEFAULT_BUDGET_TOKENS: usize = repomon_core::config::DEFAULT_BOOT_BUDGET_TOKENS;

/// The estimate: four characters to a token. Deliberately crude. The budget exists to keep a
/// system prompt from growing without bound, not to match any tokenizer.
const CHARS_PER_TOKEN: usize = 4;

/// The document's home-relative path. Daemon-owned scratch, so it lives under `.repomind/`.
pub const BOOT_REL: &str = ".repomind/boot.md";

const HEADER: &str = "# Repomind boot context\n\n\
     Assembled by the daemon from the repomind home. It is rewritten on every spawn and on\n\
     `repomind.boot`, so never hand-edit it. Live fleet state beats anything written here: read\n\
     it with the fleet tools.\n";

/// How a backend receives the boot document. Each arm is what that CLI actually supports, so a
/// release that adds a real instructions mechanism is a one-line change here plus its wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Claude: `--append-system-prompt-file <path>`, appended on top of the shipped persona
    /// (which stays the system prompt) rather than substituted for it.
    AppendSystemPromptFile,
    /// OpenCode: the path joins its config's `instructions` list, which is the CLI's own
    /// documented "additional instruction files" mechanism.
    InstructionsFile,
    /// Codex and Antigravity: neither takes a system-prompt file, so the boot document is
    /// announced as the first line typed into the composer once the session is ready.
    TypedLine,
}

/// The marker the verified composer submission watches for, exactly as fleet mail uses one.
pub const TYPED_MARKER: &str = "[END REPOMIND BOOT]";

/// Which delivery a spawned agent kind gets. Anything with no launch-time context mechanism
/// falls back to the typed line, which works on any TUI.
pub fn delivery(kind: &repomon_core::model::AgentKind) -> Delivery {
    use repomon_core::model::AgentKind::*;
    match kind {
        ClaudeCode => Delivery::AppendSystemPromptFile,
        OpenCode => Delivery::InstructionsFile,
        _ => Delivery::TypedLine,
    }
}

/// The first line typed into a [`Delivery::TypedLine`] backend's composer. The controller's cwd
/// is the home, so the home-relative path is the one the agent can open.
pub fn typed_line() -> String {
    format!(
        "Read {BOOT_REL} in your working directory before anything else. It is your boot          context: the operator's overlay, the fleet profile, the active plans, the recent          journal, and a snapshot of every lane. {TYPED_MARKER}"
    )
}

/// One lane of the fleet as the snapshot renders it. Built by the daemon from `lane.list`, and
/// passed in rather than read here so the assembly stays pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetLane {
    pub repo: String,
    /// The lane's label, e.g. `lane-7`.
    pub lane: String,
    /// The worktree's branch, or `detached` when HEAD is.
    pub branch: String,
    /// How many agent sessions are live in the lane.
    pub agents: usize,
    /// The most urgent of those agents' states, in the fleet's one-word vocabulary.
    pub status: String,
}

/// An assembled boot document and the home-relative paths the budget forced out of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootDocument {
    pub markdown: String,
    pub trimmed: Vec<String>,
}

/// `<home>/.repomind/boot.md`.
pub fn boot_path(home: &Path) -> PathBuf {
    home.join(".repomind").join("boot.md")
}

/// The document's size in the budget's own units.
pub fn tokens_estimate(text: &str) -> usize {
    text.len().div_ceil(CHARS_PER_TOKEN)
}

/// One trimmable chunk: the home-relative path that names it in the trim note, and its rendered
/// text (a whole body for a note or a journal day, one line for a plan).
#[derive(Debug, Clone)]
struct Piece {
    name: String,
    text: String,
}

/// Read the home and render the boot document, dropping pieces from the least important end
/// until the whole thing fits `budget_tokens`.
pub fn assemble_boot(home: &Path, fleet: &[FleetLane], budget_tokens: usize) -> BootDocument {
    let overlay = read_body(&home.join("REPOMIND.md"), false);
    let mut profile = markdown_pieces(home, "profile", |path, rel| {
        read_body(path, true).map(|text| Piece { name: rel, text })
    });
    let mut plans = markdown_pieces(home, "plans/active", |path, rel| {
        read_body(path, false).map(|body| Piece {
            name: rel,
            text: plan_line(path, &body),
        })
    });
    let mut journal = journal_pieces(home);

    // Least important first: the journal (and its older day before its newer one), then profile
    // notes from the end of the list, then plans. The overlay and the fleet snapshot are never
    // trimmed: the first is the operator's own instructions and the second is the only live
    // truth in the document.
    let mut order: Vec<(Section, String)> = Vec::new();
    order.extend(journal.iter().map(|p| (Section::Journal, p.name.clone())));
    order.extend(
        profile
            .iter()
            .rev()
            .map(|p| (Section::Profile, p.name.clone())),
    );
    order.extend(plans.iter().rev().map(|p| (Section::Plans, p.name.clone())));

    let mut trimmed: Vec<String> = Vec::new();
    let mut order = order.into_iter();
    loop {
        let markdown = render(
            overlay.as_deref(),
            &profile,
            &plans,
            &journal,
            fleet,
            &trimmed,
        );
        if tokens_estimate(&markdown) <= budget_tokens {
            return BootDocument { markdown, trimmed };
        }
        let Some((section, name)) = order.next() else {
            // Nothing left that may be dropped: the overlay and the fleet snapshot alone are
            // already over budget. A too-small document would be worse than an honest one.
            return BootDocument { markdown, trimmed };
        };
        let bucket = match section {
            Section::Journal => &mut journal,
            Section::Profile => &mut profile,
            Section::Plans => &mut plans,
        };
        if let Some(pos) = bucket.iter().position(|p| p.name == name) {
            trimmed.push(bucket.remove(pos).name);
        }
    }
}

/// Which list a trim candidate belongs to.
#[derive(Debug, Clone, Copy)]
enum Section {
    Journal,
    Profile,
    Plans,
}

/// Write the document to `<home>/.repomind/boot.md`, creating the scratch directory if needed.
/// Returns the path written.
pub fn write_boot(home: &Path, doc: &BootDocument) -> std::io::Result<PathBuf> {
    let path = boot_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &doc.markdown)?;
    Ok(path)
}

/// `<home>/.repomind/boot.json`: what the last regeneration produced, so `repomind.status` can
/// report it without re-reading and re-parsing the document. Daemon-owned and gitignored, like
/// the export state beside it.
pub fn state_path(home: &Path) -> PathBuf {
    home.join(".repomind").join("boot.json")
}

/// What the last regeneration produced.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BootState {
    #[serde(default)]
    pub generated_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub tokens_estimate: usize,
    #[serde(default)]
    pub trimmed: Vec<String>,
}

/// Read the boot state. A home that has never been booted reports the default, never an error.
pub fn load_state(home: &Path) -> BootState {
    std::fs::read_to_string(state_path(home))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// One regeneration: where the document landed and how big it came out.
#[derive(Debug, Clone)]
pub struct BootRun {
    pub path: PathBuf,
    pub bytes: usize,
    pub tokens_estimate: usize,
    pub trimmed: Vec<String>,
    pub generated_at: chrono::DateTime<chrono::Utc>,
}

/// How long to keep waiting for a freshly spawned CLI's composer to accept a typed line. A cold
/// Codex or Antigravity start is a few seconds; a machine under load is more.
const ANNOUNCE_WINDOW: std::time::Duration = std::time::Duration::from_secs(90);
/// How often to re-check readiness inside that window.
const ANNOUNCE_POLL: std::time::Duration = std::time::Duration::from_secs(3);
/// How many verified sends to attempt once the session looks ready. Each writes exactly one
/// supervision row, so this is deliberately small.
const ANNOUNCE_ATTEMPTS: usize = 3;

/// The managed session in `window` on `lane_id` that is safe to type a line into right now.
/// The same eligibility fleet mail uses: repomon-managed, in a real window, and with neither a
/// dialog nor a prompt already waiting on the operator.
pub fn ready_target<'a>(
    lanes: &'a [repomon_core::model::Lane],
    lane_id: repomon_core::model::LaneId,
    window: &str,
) -> Option<&'a repomon_core::model::AgentSession> {
    lanes
        .iter()
        .find(|lane| lane.id == lane_id)?
        .agent_sessions
        .iter()
        .find(|s| s.tmux_window.as_deref() == Some(window) && crate::mail::injection_eligible(s))
}

/// Announce the boot document to a backend with no launch-time context mechanism, by typing one
/// line into its composer once the session is ready.
///
/// Backgrounded on purpose: a spawn must return as soon as the window exists, and the CLI behind
/// it takes seconds to draw a composer. Delivery goes through the same verified injection fleet
/// mail uses, so it can never type over a busy composer, and it gives up quietly rather than
/// retrying forever into a window that never became ready.
pub fn announce_typed_line(
    ctx: std::sync::Arc<crate::Ctx>,
    lane_id: repomon_core::model::LaneId,
    window: String,
    agent_kind: Option<String>,
) {
    tokio::spawn(async move {
        let deadline = std::time::Instant::now() + ANNOUNCE_WINDOW;
        let mut attempts = 0;
        while std::time::Instant::now() < deadline && attempts < ANNOUNCE_ATTEMPTS {
            tokio::time::sleep(ANNOUNCE_POLL).await;
            let lanes = ctx.lanes.list().await.unwrap_or_default();
            let Some(session) = ready_target(&lanes, lane_id, &window) else {
                continue;
            };
            let seed = crate::inject::AuditSeed {
                lane_id,
                window: window.clone(),
                session_id: session.session_id.clone(),
                agent_kind: agent_kind.clone(),
                trigger: "boot".to_string(),
                dialog_class: None,
                repo_scoped: None,
                decision: "boot_context".to_string(),
                policy_source: None,
                reason: Some("repomind boot context".to_string()),
                subject: None,
                pane_excerpt: None,
            };
            attempts += 1;
            match crate::inject::verified_send(
                &ctx,
                crate::inject::Expectation::IdleNoDialog,
                crate::inject::Payload::VerifiedLine {
                    text: typed_line(),
                    marker: TYPED_MARKER.to_string(),
                },
                seed,
            )
            .await
            {
                crate::inject::SendOutcome::Sent { .. } => {
                    tracing::info!("repomind boot context announced to {window}");
                    return;
                }
                crate::inject::SendOutcome::Skipped { reason, .. } => {
                    if reason == crate::inject::SkipReason::WindowGone {
                        return;
                    }
                }
                crate::inject::SendOutcome::Failed { error, .. } => {
                    tracing::warn!("repomind boot announcement to {window} failed: {error}");
                    return;
                }
            }
        }
        tracing::debug!("repomind boot context was never announced to {window}");
    });
}

/// Regenerate `.repomind/boot.md` from the live fleet and the home's own files, and record what
/// it produced. Called on every spawn into the controller lane and by the `repomind.boot` RPC;
/// a spawn must never fail because the boot document could not be written, so callers log
/// rather than propagate.
pub async fn regenerate(ctx: &crate::Ctx) -> repomon_core::Result<BootRun> {
    let (home, budget) = {
        let cfg = ctx.config.read().await;
        (cfg.repomind_home(), cfg.repomind.boot_budget_tokens)
    };
    // Live truth, read the same way any client reads it. A lane listing that fails (no repos
    // registered yet on a first start) yields an empty snapshot rather than no document.
    let fleet = fleet_snapshot(&ctx.lanes.list().await.unwrap_or_default());
    let generated_at = chrono::Utc::now();

    tokio::task::spawn_blocking(move || -> std::io::Result<BootRun> {
        let doc = assemble_boot(&home, &fleet, budget);
        let path = write_boot(&home, &doc)?;
        let state = BootState {
            generated_at: Some(generated_at),
            tokens_estimate: tokens_estimate(&doc.markdown),
            trimmed: doc.trimmed.clone(),
        };
        let body = serde_json::to_string_pretty(&state).map_err(std::io::Error::other)?;
        std::fs::write(state_path(&home), body + "\n")?;
        Ok(BootRun {
            path,
            bytes: doc.markdown.len(),
            tokens_estimate: state.tokens_estimate,
            trimmed: doc.trimmed,
            generated_at,
        })
    })
    .await
    .map_err(|e| repomon_core::Error::Other(e.to_string()))?
    .map_err(repomon_core::Error::Io)
}

/// Read a markdown file, optionally stripping its frontmatter. `None` when it is missing or has
/// nothing but whitespace in it.
fn read_body(path: &Path, strip_frontmatter: bool) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let body = if strip_frontmatter {
        md::split_frontmatter(&raw).1
    } else {
        raw
    };
    let body = body.trim();
    (!body.is_empty()).then(|| body.to_string())
}

/// Every `*.md` directly in `<home>/<rel_dir>`, sorted by name, mapped through `make`.
fn markdown_pieces(
    home: &Path,
    rel_dir: &str,
    make: impl Fn(&Path, String) -> Option<Piece>,
) -> Vec<Piece> {
    let dir = home.join(rel_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md") && !n.eq_ignore_ascii_case("README.md"))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|name| {
            let rel = format!("{rel_dir}/{name}");
            make(&dir.join(&name), rel)
        })
        .collect()
}

/// Yesterday's and today's day files, in that order. Older days live in the archive and are not
/// boot context; a controller that wants them reads the files.
fn journal_pieces(home: &Path) -> Vec<Piece> {
    let today = Local::now().date_naive();
    [today - Duration::days(1), today]
        .into_iter()
        .filter_map(|d| {
            let rel = format!("journal/{}.md", d.format("%Y-%m-%d"));
            read_body(&home.join(&rel), true).map(|text| Piece { name: rel, text })
        })
        .collect()
}

/// Reduce one active plan to a single status line: what it is, where it stands, who owns it, and
/// what happens next. The body itself never reaches the boot document.
fn plan_line(path: &Path, raw: &str) -> String {
    let (frontmatter, body) = md::split_frontmatter(raw);
    let fm = frontmatter.unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "plan".to_string());
    let title = md::field(&fm, "title")
        .or_else(|| body_field(&body, "title"))
        .or_else(|| {
            body.lines()
                .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        })
        .unwrap_or(stem);
    let status = md::field(&fm, "status")
        .or_else(|| body_field(&body, "status"))
        .unwrap_or_else(|| "active".to_string());
    let owner = md::field(&fm, "owner")
        .or_else(|| body_field(&body, "owner"))
        .unwrap_or_else(|| "unassigned".to_string());
    let next = body_field(&body, "next step")
        .or_else(|| {
            body.lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.trim_start_matches(['-', '*', ' ']).to_string())
        })
        .unwrap_or_else(|| "no next step recorded".to_string());
    format!("- {title}: {status}, owner {owner}, next: {next}")
}

/// A `Key: value` line in a body, with an optional list-marker prefix. Case-insensitive on the
/// key, because the operator writes these by hand.
fn body_field(body: &str, key: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
        let (found, rest) = line.split_once(':')?;
        (found.trim().eq_ignore_ascii_case(key) && !rest.trim().is_empty())
            .then(|| rest.trim().to_string())
    })
}

/// One fleet line per lane. `1 agent`, not `1 agents`.
fn fleet_line(lane: &FleetLane) -> String {
    let unit = if lane.agents == 1 { "agent" } else { "agents" };
    format!(
        "- {} {} ({}): {} {}, {}",
        lane.repo, lane.lane, lane.branch, lane.agents, unit, lane.status
    )
}

/// The most urgent state among a lane's live agents, in the fleet's one-word vocabulary. An
/// empty lane says so rather than pretending to be idle.
fn most_urgent(sessions: &[repomon_core::model::AgentSession]) -> String {
    use repomon_core::model::AgentStatus::*;
    // Descending urgency: a lane with one waiting agent needs the operator whatever else runs.
    for status in [Waiting, RateLimited, Running, Idle, Ended] {
        if sessions.iter().any(|s| s.status == status) {
            return status.as_str().to_string();
        }
    }
    "no agents".to_string()
}

/// Reduce the live lane list to one snapshot line per lane, ordered by repo then lane so the
/// document is stable between spawns.
pub fn fleet_snapshot(lanes: &[repomon_core::model::Lane]) -> Vec<FleetLane> {
    let mut out: Vec<FleetLane> = lanes
        .iter()
        .map(|lane| FleetLane {
            repo: lane.repo.name.clone(),
            lane: format!("lane-{}", lane.id),
            branch: lane
                .worktree
                .branch
                .clone()
                .unwrap_or_else(|| "detached".to_string()),
            agents: lane.agent_sessions.len(),
            status: most_urgent(&lane.agent_sessions),
        })
        .collect();
    out.sort_by(|a, b| a.repo.cmp(&b.repo).then(a.lane.cmp(&b.lane)));
    out
}

/// Render the document from whatever survived the budget. Every section is omitted when empty,
/// so an empty home yields the header and nothing else.
fn render(
    overlay: Option<&str>,
    profile: &[Piece],
    plans: &[Piece],
    journal: &[Piece],
    fleet: &[FleetLane],
    trimmed: &[String],
) -> String {
    let mut out = String::from(HEADER);
    if let Some(overlay) = overlay {
        out.push_str("\n## Operator overlay\n\n");
        out.push_str(overlay);
        out.push('\n');
    }
    if !profile.is_empty() {
        out.push_str("\n## Profile\n");
        for piece in profile {
            out.push_str(&format!("\n### {}\n\n{}\n", piece.name, piece.text));
        }
    }
    if !plans.is_empty() {
        out.push_str("\n## Active plans\n\n");
        for piece in plans {
            out.push_str(&piece.text);
            out.push('\n');
        }
    }
    if !journal.is_empty() {
        out.push_str("\n## Journal\n");
        for piece in journal {
            out.push_str(&format!("\n### {}\n\n{}\n", piece.name, piece.text));
        }
    }
    if !fleet.is_empty() {
        out.push_str("\n## Fleet snapshot\n\n");
        for lane in fleet {
            out.push_str(&fleet_line(lane));
            out.push('\n');
        }
    }
    if !trimmed.is_empty() {
        out.push_str(&format!("\nTrimmed: {}\n", trimmed.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Local};
    use std::path::{Path, PathBuf};

    fn write(home: &Path, rel: &str, body: &str) {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        super::super::ensure_layout(&home).unwrap();
        std::fs::remove_file(home.join("REPOMIND.md")).unwrap();
        (dir, home)
    }

    fn day(offset: i64) -> String {
        (Local::now().date_naive() + Duration::days(offset))
            .format("%Y-%m-%d")
            .to_string()
    }

    fn lane() -> FleetLane {
        FleetLane {
            repo: "repomon".into(),
            lane: "lane-7".into(),
            branch: "feat/repomind-boot".into(),
            agents: 2,
            status: "waiting".into(),
        }
    }

    #[test]
    fn the_document_orders_overlay_profile_plans_journal_then_fleet() {
        let (_dir, home) = home();
        write(&home, "REPOMIND.md", "# Overlay\n\nhouse rules\n");
        write(
            &home,
            "profile/fleet.md",
            "---\ntitle: Fleet\n---\n\nthe operator runs six repos\n",
        );
        write(
            &home,
            "plans/active/ship-r3.md",
            "---\ntitle: Ship R3\nstatus: in flight\n---\n\nbody\n",
        );
        write(
            &home,
            &format!("journal/{}.md", day(0)),
            "# Journal\n\ntoday happened\n",
        );

        let doc = assemble_boot(&home, &[lane()], DEFAULT_BUDGET_TOKENS);

        let at = |needle: &str| {
            doc.markdown
                .find(needle)
                .unwrap_or_else(|| panic!("missing {needle} in\n{}", doc.markdown))
        };
        assert!(at("house rules") < at("the operator runs six repos"));
        assert!(at("the operator runs six repos") < at("Ship R3"));
        assert!(at("Ship R3") < at("today happened"));
        assert!(at("today happened") < at("feat/repomind-boot"));
        assert!(doc.trimmed.is_empty(), "{:?}", doc.trimmed);
    }

    #[test]
    fn profile_notes_contribute_their_body_without_frontmatter() {
        let (_dir, home) = home();
        write(
            &home,
            "profile/quotas.md",
            "---\ntitle: Quotas\npermalink: repomind/profile/quotas\n---\n\ntwo Claude accounts\n",
        );

        let doc = assemble_boot(&home, &[], DEFAULT_BUDGET_TOKENS);

        assert!(doc.markdown.contains("two Claude accounts"));
        assert!(
            !doc.markdown.contains("permalink:"),
            "frontmatter leaked:\n{}",
            doc.markdown
        );
    }

    #[test]
    fn an_active_plan_is_reduced_to_one_line_carrying_its_next_step() {
        let (_dir, home) = home();
        write(
            &home,
            "plans/active/ship-r3.md",
            "---\ntitle: Ship R3\nstatus: in flight\nowner: lane-7/1\n---\n\n# Ship R3\n\nA long body that must not reach the boot document at all.\n\nNext step: land the boot assembly\n\nMore prose nobody needs.\n",
        );

        let doc = assemble_boot(&home, &[], DEFAULT_BUDGET_TOKENS);

        let line = doc
            .markdown
            .lines()
            .find(|l| l.contains("Ship R3"))
            .expect("a plan line");
        assert!(line.contains("in flight"), "{line}");
        assert!(line.contains("lane-7/1"), "{line}");
        assert!(line.contains("land the boot assembly"), "{line}");
        assert!(!doc.markdown.contains("More prose nobody needs"));
    }

    #[test]
    fn only_yesterday_and_today_journal_days_are_read() {
        let (_dir, home) = home();
        write(
            &home,
            &format!("journal/{}.md", day(-9)),
            "ancient history\n",
        );
        write(
            &home,
            &format!("journal/{}.md", day(-1)),
            "yesterday happened\n",
        );
        write(&home, &format!("journal/{}.md", day(0)), "today happened\n");

        let doc = assemble_boot(&home, &[], DEFAULT_BUDGET_TOKENS);

        assert!(doc.markdown.contains("yesterday happened"));
        assert!(doc.markdown.contains("today happened"));
        assert!(!doc.markdown.contains("ancient history"));
    }

    #[test]
    fn the_fleet_snapshot_is_one_line_per_lane() {
        let (_dir, home) = home();
        let lanes = [
            lane(),
            FleetLane {
                repo: "docuchat".into(),
                lane: "lane-9".into(),
                branch: "main".into(),
                agents: 1,
                status: "running".into(),
            },
        ];

        let doc = assemble_boot(&home, &lanes, DEFAULT_BUDGET_TOKENS);

        let lines: Vec<&str> = doc
            .markdown
            .lines()
            .filter(|l| l.starts_with("- repomon ") || l.starts_with("- docuchat "))
            .collect();
        assert_eq!(lines.len(), 2, "{:?}", lines);
        assert!(
            lines[0].contains("feat/repomind-boot")
                && lines[0].contains("2 agents")
                && lines[0].contains("waiting")
        );
        assert!(
            lines[1].contains("1 agent,"),
            "singular agent count: {}",
            lines[1]
        );
    }

    /// The bound is hard: over budget, the journal goes first, then profile, then plans, and the
    /// document ends by naming exactly what it dropped.
    #[test]
    fn a_tiny_budget_trims_the_journal_first_and_names_what_was_cut() {
        let (_dir, home) = home();
        write(&home, "REPOMIND.md", "# Overlay\n\nhouse rules\n");
        write(
            &home,
            "profile/fleet.md",
            &format!(
                "---\ntitle: Fleet\n---\n\n{}\n",
                "profile prose. ".repeat(80)
            ),
        );
        write(
            &home,
            "plans/active/ship-r3.md",
            "---\ntitle: Ship R3\nstatus: in flight\n---\n\nNext step: land it\n",
        );
        write(
            &home,
            &format!("journal/{}.md", day(0)),
            &format!("{}\n", "journal prose. ".repeat(80)),
        );

        let doc = assemble_boot(&home, &[lane()], 450);

        assert_eq!(doc.trimmed, vec![format!("journal/{}.md", day(0))]);
        assert!(
            tokens_estimate(&doc.markdown) <= 450,
            "{} tokens",
            tokens_estimate(&doc.markdown)
        );
        assert!(
            doc.markdown.contains("house rules"),
            "the overlay is never trimmed"
        );
        assert!(doc.markdown.contains("Ship R3"), "plans are trimmed last");
        assert!(
            doc.markdown
                .trim_end()
                .ends_with(&format!("Trimmed: journal/{}.md", day(0))),
            "tail was:\n{}",
            doc.markdown
        );
    }

    #[test]
    fn a_budget_smaller_than_the_home_trims_profile_and_plans_too() {
        let (_dir, home) = home();
        write(
            &home,
            "profile/fleet.md",
            &format!("{}\n", "profile prose. ".repeat(80)),
        );
        write(
            &home,
            "plans/active/ship-r3.md",
            "---\ntitle: Ship R3\n---\n\nNext step: land it\n",
        );
        write(
            &home,
            &format!("journal/{}.md", day(0)),
            &format!("{}\n", "journal prose. ".repeat(80)),
        );

        let doc = assemble_boot(&home, &[], 30);

        assert_eq!(
            doc.trimmed,
            vec![
                format!("journal/{}.md", day(0)),
                "profile/fleet.md".to_string(),
                "plans/active/ship-r3.md".to_string(),
            ]
        );
    }

    #[test]
    fn an_empty_home_yields_a_minimal_document() {
        let (_dir, home) = home();

        let doc = assemble_boot(&home, &[], DEFAULT_BUDGET_TOKENS);

        assert!(doc.markdown.starts_with("# Repomind boot context"));
        assert!(doc.trimmed.is_empty());
        for absent in [
            "## Operator overlay",
            "## Profile",
            "## Active plans",
            "## Journal",
            "## Fleet snapshot",
        ] {
            assert!(
                !doc.markdown.contains(absent),
                "{absent} should be omitted:\n{}",
                doc.markdown
            );
        }
    }

    #[test]
    fn a_home_that_does_not_exist_still_assembles() {
        let dir = tempfile::tempdir().unwrap();
        let doc = assemble_boot(&dir.path().join("nope"), &[], DEFAULT_BUDGET_TOKENS);
        assert!(doc.markdown.starts_with("# Repomind boot context"));
    }

    #[test]
    fn writing_the_boot_document_creates_the_daemon_owned_scratch_dir() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("repomind");
        let doc = assemble_boot(&home, &[], DEFAULT_BUDGET_TOKENS);

        let path = write_boot(&home, &doc).unwrap();

        assert_eq!(path, home.join(".repomind").join("boot.md"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), doc.markdown);
    }

    fn session(status: repomon_core::model::AgentStatus) -> repomon_core::model::AgentSession {
        repomon_core::model::AgentSession {
            id: 1,
            agent: repomon_core::model::AgentKind::ClaudeCode,
            repo_id: 1,
            worktree_id: Some(1),
            started_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            ended_at: None,
            manifest_path: PathBuf::from("/m"),
            tool_call_count: 0,
            title: None,
            status,
            external: false,
            session_id: None,
            resume_at: None,
            inferred: false,
            tmux_window: None,
            pending_dialog: None,
            stale: false,
            stalled_since: None,
            status_reason: None,
            attention_kind: None,
            ended_turn: false,
            gate: None,
            config_dir: None,
            custom_label: None,
            generated_label: None,
            last_message: None,
            pending_prompt: None,
            subagent_running: None,
        }
    }

    fn test_lane(
        id: repomon_core::model::LaneId,
        repo: &str,
        branch: Option<&str>,
        sessions: Vec<repomon_core::model::AgentSession>,
    ) -> repomon_core::model::Lane {
        let head = "0000000000000000000000000000000000000000".parse().unwrap();
        repomon_core::model::Lane {
            id,
            repo: repomon_core::model::Repo {
                id: 1,
                name: repo.into(),
                path: PathBuf::from("/r"),
                added_at: chrono::Utc::now(),
                worktree_root_template: None,
                hidden: false,
                position: None,
                label: None,
            },
            worktree: repomon_core::model::Worktree {
                id: 1,
                repo_id: 1,
                path: PathBuf::from("/r"),
                branch: branch.map(str::to_string),
                head,
                is_main: true,
                name: "r".into(),
            },
            state: repomon_core::model::WorktreeState {
                worktree_id: 1,
                head,
                branch: branch.map(str::to_string),
                upstream: None,
                ahead: 0,
                behind: 0,
                dirty: Default::default(),
                last_commit_at: None,
                locked: false,
                prunable: false,
                merged: false,
                last_change_at: None,
            },
            agent_sessions: sessions,
            last_activity_at: chrono::Utc::now(),
            pinned: false,
            role: None,
        }
    }

    /// The snapshot is the only live truth in the boot document, so it reports what a lane's
    /// most urgent agent is doing, not just how many there are.
    #[test]
    fn the_snapshot_reports_each_lanes_most_urgent_agent_state() {
        use repomon_core::model::AgentStatus;
        let lanes = [test_lane(
            7,
            "repomon",
            Some("feat/x"),
            vec![session(AgentStatus::Running), session(AgentStatus::Waiting)],
        )];

        assert_eq!(
            fleet_snapshot(&lanes),
            vec![FleetLane {
                repo: "repomon".into(),
                lane: "lane-7".into(),
                branch: "feat/x".into(),
                agents: 2,
                status: "waiting".into(),
            }]
        );
    }

    #[test]
    fn a_lane_with_no_agents_and_a_detached_head_still_gets_a_line() {
        let lanes = [test_lane(3, "docuchat", None, Vec::new())];

        assert_eq!(
            fleet_snapshot(&lanes),
            vec![FleetLane {
                repo: "docuchat".into(),
                lane: "lane-3".into(),
                branch: "detached".into(),
                agents: 0,
                status: "no agents".into(),
            }]
        );
    }

    #[test]
    fn snapshot_lanes_are_ordered_by_repo_then_lane() {
        let lanes = [
            test_lane(9, "repomon", Some("main"), Vec::new()),
            test_lane(2, "avenith", Some("main"), Vec::new()),
            test_lane(4, "repomon", Some("main"), Vec::new()),
        ];

        let names: Vec<String> = fleet_snapshot(&lanes).into_iter().map(|l| l.lane).collect();

        assert_eq!(names, vec!["lane-2", "lane-4", "lane-9"]);
    }

    #[test]
    fn each_backend_gets_the_delivery_its_cli_supports() {
        use repomon_core::model::AgentKind;
        assert_eq!(
            delivery(&AgentKind::ClaudeCode),
            Delivery::AppendSystemPromptFile
        );
        assert_eq!(delivery(&AgentKind::OpenCode), Delivery::InstructionsFile);
        assert_eq!(delivery(&AgentKind::Codex), Delivery::TypedLine);
        assert_eq!(delivery(&AgentKind::Antigravity), Delivery::TypedLine);
        // A CLI with no launch-time context mechanism still gets told where to look.
        assert_eq!(
            delivery(&AgentKind::Other("hermes".into())),
            Delivery::TypedLine
        );
    }

    /// The typed line names the file by its home-relative path (the controller's cwd is the
    /// home) and ends with the marker the verified composer submission watches for.
    #[test]
    fn the_typed_line_names_the_boot_file_and_carries_its_marker() {
        let line = typed_line();
        assert!(
            line.starts_with(
                "Read .repomind/boot.md in your working directory before anything else."
            ),
            "{line}"
        );
        assert!(line.ends_with(TYPED_MARKER), "{line}");
    }

    /// The typed line is only ever sent into a session that is actually ready for it: the right
    /// lane, the right window, managed by repomon, and with no dialog already waiting.
    #[test]
    fn the_typed_line_target_is_the_managed_session_in_that_window() {
        use repomon_core::model::AgentStatus;
        let mut ready = session(AgentStatus::Idle);
        ready.tmux_window = Some("lane-7-1".into());
        let lanes = [test_lane(7, "repomind", Some("main"), vec![ready])];

        assert!(ready_target(&lanes, 7, "lane-7-1").is_some());
        assert!(
            ready_target(&lanes, 7, "lane-7-2").is_none(),
            "wrong window"
        );
        assert!(ready_target(&lanes, 9, "lane-7-1").is_none(), "wrong lane");
    }

    #[test]
    fn a_session_with_a_dialog_waiting_is_not_a_typed_line_target() {
        use repomon_core::model::AgentStatus;
        let mut busy = session(AgentStatus::Waiting);
        busy.tmux_window = Some("lane-7-1".into());
        busy.pending_dialog = Some(repomon_core::agent::prompt::PendingDialog {
            title: None,
            question: "Allow?".into(),
            body: Vec::new(),
            options: Vec::new(),
            selected: None,
            context: Vec::new(),
        });
        let lanes = [test_lane(7, "repomind", Some("main"), vec![busy])];

        assert!(ready_target(&lanes, 7, "lane-7-1").is_none());
    }

    #[test]
    fn tokens_are_estimated_at_four_characters_each() {
        assert_eq!(tokens_estimate(""), 0);
        assert_eq!(tokens_estimate("abcd"), 1);
        assert_eq!(tokens_estimate("abcde"), 2);
    }
}
