//! Prices recorded token usage at query time. Unknown models retain their token counts and are
//! reported as unpriced.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{LaneId, RepoId};
use crate::pricing::{PriceTable, TokenCounts};

pub mod scan;

pub use scan::{HEADLINE_VERSION, UNTITLED_SESSION};

/// Identifies the reader revision, whose change schedules bounded source replay and replacement of
/// previously stored events.
pub const INGEST_VERSION: u32 = 1;

/// One stored ledger row.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    pub at: DateTime<Utc>,
    pub agent_kind: String,
    pub model: String,
    pub account: String,
    pub lane_id: Option<LaneId>,
    pub repo_id: Option<RepoId>,
    pub session_id: Option<String>,
    /// The tmux window the lane runs in, when the turn was attributed to one.
    pub window: Option<String>,
    pub cwd: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub thinking_tokens: u64,
    /// Whether the counts were estimated rather than reported by the agent.
    pub estimated: bool,
    /// Whether the session ran outside repomon's control.
    pub external: bool,
    /// Whether the turn ran in a subagent the session spawned rather than in the session itself.
    pub subagent: bool,
    pub source_path: String,
    pub source_offset: i64,
}

impl UsageEvent {
    /// The counts this row contributes to a price calculation.
    pub fn tokens(&self) -> TokenCounts {
        TokenCounts {
            input: self.input_tokens,
            output: self.output_tokens,
            cache_read: self.cache_read_tokens,
            cache_write: self.cache_write_tokens,
        }
    }

    /// Every token the row accounts for, cached reads included.
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

/// Which repo and lane a turn belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Attribution {
    pub lane_id: Option<LaneId>,
    pub repo_id: Option<RepoId>,
    /// True when repomon did not run this session: no lane owns the directory it ran in.
    pub external: bool,
}

/// The repo and lane paths a scan is attributed against. Built once per ingest pass.
#[derive(Debug, Clone, Default)]
pub struct FleetIndex {
    repos: Vec<(RepoId, PathBuf)>,
    lanes: Vec<(LaneId, RepoId, PathBuf)>,
}

impl FleetIndex {
    /// Build an index from `(repo_id, repo path)` and `(lane_id, repo_id, worktree path)` pairs.
    pub fn new(repos: Vec<(RepoId, PathBuf)>, lanes: Vec<(LaneId, RepoId, PathBuf)>) -> Self {
        FleetIndex { repos, lanes }
    }

    /// Attribute a working directory. The longest matching lane worktree wins, because a lane's
    /// worktree usually sits inside or beside its repo and the deeper path is the more specific
    /// answer. With no lane match the repo still owns the row, flagged external.
    pub fn attribute(&self, cwd: Option<&str>) -> Attribution {
        let cwd = match cwd {
            Some(c) if !c.is_empty() => Path::new(c).to_path_buf(),
            _ => {
                return Attribution {
                    external: true,
                    ..Default::default()
                };
            }
        };
        let mut best: Option<(&LaneId, &RepoId, usize)> = None;
        for (lane, repo, root) in &self.lanes {
            if let Some(len) = prefix_len(&cwd, root) {
                if best.is_none_or(|(_, _, b)| len > b) {
                    best = Some((lane, repo, len));
                }
            }
        }
        if let Some((lane, repo, _)) = best {
            return Attribution {
                lane_id: Some(*lane),
                repo_id: Some(*repo),
                external: false,
            };
        }
        let mut repo_best: Option<(&RepoId, usize)> = None;
        for (repo, root) in &self.repos {
            if let Some(len) = prefix_len(&cwd, root) {
                if repo_best.is_none_or(|(_, b)| len > b) {
                    repo_best = Some((repo, len));
                }
            }
        }
        Attribution {
            lane_id: None,
            repo_id: repo_best.map(|(r, _)| *r),
            external: true,
        }
    }
}

/// How many components of `root` prefix `cwd`, or `None` when it does not.
fn prefix_len(cwd: &Path, root: &Path) -> Option<usize> {
    if cwd.starts_with(root) {
        Some(root.components().count())
    } else {
        None
    }
}

/// The dimension a summary or timeline splits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, rename = "UsageGroupBy"))]
pub enum GroupBy {
    #[default]
    Kind,
    Model,
    Repo,
    Lane,
    Account,
}

impl GroupBy {
    fn key_of(self, e: &UsageEvent) -> String {
        match self {
            GroupBy::Kind => e.agent_kind.clone(),
            GroupBy::Model => e.model.clone(),
            GroupBy::Repo => e.repo_id.map(|r| r.to_string()).unwrap_or_default(),
            GroupBy::Lane => e.lane_id.map(|l| l.to_string()).unwrap_or_default(),
            GroupBy::Account => e.account.clone(),
        }
    }
}

/// A timeline's bucket width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, rename = "UsageBucket"))]
pub enum Bucket {
    /// Fifteen minutes.
    Quarter,
    #[default]
    Hour,
    Day,
}

impl Bucket {
    /// The start of the bucket an instant falls in.
    pub fn floor(self, at: DateTime<Utc>) -> DateTime<Utc> {
        let (h, m) = match self {
            Bucket::Quarter => (at.hour(), at.minute() - at.minute() % 15),
            Bucket::Hour => (at.hour(), 0),
            Bucket::Day => (0, 0),
        };
        Utc.with_ymd_and_hms(at.year(), at.month(), at.day(), h, m, 0)
            .single()
            .unwrap_or(at)
    }
}

/// A named window of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, rename = "UsageRange"))]
pub enum Range {
    #[default]
    Today,
    /// The last seven days, today included.
    Week,
    /// The last thirty days, today included.
    Month,
    /// A window the caller supplies as `since` and `until`.
    Custom,
}

impl Range {
    /// Returns inclusive range bounds using the daemon’s local midnight for day-aligned names;
    /// clients in another zone must send explicit bounds.
    pub fn window(self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let back = match self {
            Range::Today => 0,
            Range::Week => 6,
            Range::Month => 29,
            Range::Custom => 0,
        };
        let local = now.with_timezone(&Local);
        let day = local.date_naive() - Duration::days(back);
        // A day that has no midnight (a spring-forward transition at 00:00, as in Santiago) still
        // has a first instant, and that is where its total starts.
        let start = day
            .and_hms_opt(0, 0, 0)
            .and_then(|naive| {
                Local
                    .from_local_datetime(&naive)
                    .earliest()
                    .or_else(|| Local.from_local_datetime(&naive).latest())
            })
            .map(|at| at.with_timezone(&Utc))
            .unwrap_or(now);
        (start, now)
    }
}

/// Token and cost totals for a set of rows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageTotals {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub input_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub output_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_read_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_write_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub thinking_tokens: u64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_tokens: u64,
    /// Of `total_tokens`, how many came from an estimate rather than a reported count.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub estimated_tokens: u64,
    /// Of `total_tokens`, how many a subagent spent rather than the session's own turns.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub subagent_tokens: u64,
    pub cost_usd: f64,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub events: u64,
}

impl UsageTotals {
    fn add(&mut self, e: &UsageEvent, table: &PriceTable) {
        self.input_tokens += e.input_tokens;
        self.output_tokens += e.output_tokens;
        self.cache_read_tokens += e.cache_read_tokens;
        self.cache_write_tokens += e.cache_write_tokens;
        self.thinking_tokens += e.thinking_tokens;
        self.total_tokens += e.total_tokens();
        if e.estimated {
            self.estimated_tokens += e.total_tokens();
        }
        if e.subagent {
            self.subagent_tokens += e.total_tokens();
        }
        self.cost_usd += table.cost(&e.model, e.at, &e.tokens()).unwrap_or(0.0);
        self.events += 1;
    }
}

/// One row of a summary breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageGroupRow {
    /// The raw group value: an agent kind, a model id, or a repo or lane id as text.
    pub key: String,
    /// What to show for `key`. Equal to `key` until the daemon resolves repo and lane names.
    pub label: String,
    pub totals: UsageTotals,
    /// True when at least one event in this group has no published price. Its tokens are still
    /// counted; its share of `cost_usd` reads as zero rather than an estimate. A free-tier id
    /// (which really does cost zero) never sets this.
    pub unpriced: bool,
}

/// The answer to `usage.summary`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageSummary {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub group_by: GroupBy,
    pub totals: UsageTotals,
    pub groups: Vec<UsageGroupRow>,
    /// Cached reads as a share of all read-side tokens, between zero and one.
    pub cache_hit_rate: f64,
    /// Estimated tokens as a share of all tokens, between zero and one.
    pub estimated_share: f64,
    /// Models the price table had no rate for. Their tokens count; their cost reads as zero.
    pub unpriced_models: Vec<String>,
}

/// Summarize events, splitting on `group_by` and pricing through `table`.
pub fn summarize(events: &[UsageEvent], group_by: GroupBy, table: &PriceTable) -> UsageSummary {
    let mut totals = UsageTotals::default();
    let mut groups: BTreeMap<String, UsageTotals> = BTreeMap::new();
    let mut group_unpriced: BTreeMap<String, bool> = BTreeMap::new();
    let mut unpriced: BTreeMap<String, ()> = BTreeMap::new();
    for e in events {
        totals.add(e, table);
        let key = group_by.key_of(e);
        groups.entry(key.clone()).or_default().add(e, table);
        // A free-tier id prices at zero, so it belongs in the totals and not in the warning.
        let has_no_price = table.lookup(&e.model, e.at).is_none()
            && !e.model.is_empty()
            && !crate::pricing::is_free_tier(&e.model);
        if has_no_price {
            unpriced.insert(e.model.clone(), ());
            group_unpriced.insert(key, true);
        } else {
            group_unpriced.entry(key).or_insert(false);
        }
    }
    let read_side = totals.input_tokens + totals.cache_read_tokens;
    let cache_hit_rate = if read_side == 0 {
        0.0
    } else {
        totals.cache_read_tokens as f64 / read_side as f64
    };
    let estimated_share = if totals.total_tokens == 0 {
        0.0
    } else {
        totals.estimated_tokens as f64 / totals.total_tokens as f64
    };
    let mut groups: Vec<UsageGroupRow> = groups
        .into_iter()
        .map(|(key, totals)| {
            let unpriced = group_unpriced.get(&key).copied().unwrap_or(false);
            UsageGroupRow {
                label: key.clone(),
                key,
                totals,
                unpriced,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.totals
            .cost_usd
            .total_cmp(&a.totals.cost_usd)
            .then_with(|| b.totals.total_tokens.cmp(&a.totals.total_tokens))
            .then_with(|| a.key.cmp(&b.key))
    });
    let (from, to) = window_of(events);
    UsageSummary {
        from,
        to,
        group_by,
        totals,
        groups,
        cache_hit_rate,
        estimated_share,
        unpriced_models: unpriced.into_keys().collect(),
    }
}

fn window_of(events: &[UsageEvent]) -> (DateTime<Utc>, DateTime<Utc>) {
    let from = events.iter().map(|e| e.at).min().unwrap_or_else(Utc::now);
    let to = events.iter().map(|e| e.at).max().unwrap_or(from);
    (from, to)
}

/// One point on a timeline series.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsagePoint {
    pub at: DateTime<Utc>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub total_tokens: u64,
    pub cost_usd: f64,
}

/// One series of a timeline, one per group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageSeries {
    pub key: String,
    pub label: String,
    pub points: Vec<UsagePoint>,
    pub totals: UsageTotals,
}

/// The answer to `usage.timeline`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageTimeline {
    pub bucket: Bucket,
    pub group_by: GroupBy,
    pub series: Vec<UsageSeries>,
    /// Every bucket start any series has a point in, oldest first, so a chart can share an axis.
    pub buckets: Vec<DateTime<Utc>>,
}

/// Bucket events into per-group series. Series carry only the buckets they have data in; the
/// shared `buckets` axis is the union, so a chart can decide how to render a gap.
pub fn timeline(
    events: &[UsageEvent],
    bucket: Bucket,
    group_by: GroupBy,
    table: &PriceTable,
) -> UsageTimeline {
    let mut by: BTreeMap<String, BTreeMap<DateTime<Utc>, UsageTotals>> = BTreeMap::new();
    let mut axis: BTreeMap<DateTime<Utc>, ()> = BTreeMap::new();
    for e in events {
        let slot = bucket.floor(e.at);
        axis.insert(slot, ());
        by.entry(group_by.key_of(e))
            .or_default()
            .entry(slot)
            .or_default()
            .add(e, table);
    }
    let mut series: Vec<UsageSeries> = by
        .into_iter()
        .map(|(key, points)| {
            let mut totals = UsageTotals::default();
            let points: Vec<UsagePoint> = points
                .into_iter()
                .map(|(at, t)| {
                    totals.input_tokens += t.input_tokens;
                    totals.output_tokens += t.output_tokens;
                    totals.cache_read_tokens += t.cache_read_tokens;
                    totals.cache_write_tokens += t.cache_write_tokens;
                    totals.thinking_tokens += t.thinking_tokens;
                    totals.total_tokens += t.total_tokens;
                    totals.estimated_tokens += t.estimated_tokens;
                    totals.subagent_tokens += t.subagent_tokens;
                    totals.cost_usd += t.cost_usd;
                    totals.events += t.events;
                    UsagePoint {
                        at,
                        total_tokens: t.total_tokens,
                        cost_usd: t.cost_usd,
                    }
                })
                .collect();
            UsageSeries {
                label: key.clone(),
                key,
                points,
                totals,
            }
        })
        .collect();
    series.sort_by(|a, b| {
        b.totals
            .cost_usd
            .total_cmp(&a.totals.cost_usd)
            .then_with(|| a.key.cmp(&b.key))
    });
    UsageTimeline {
        bucket,
        group_by,
        series,
        buckets: axis.into_keys().collect(),
    }
}

/// One row of `usage.sessions`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageSessionRow {
    pub session_id: String,
    pub agent_kind: String,
    /// The model the session spent the most tokens on.
    pub model: String,
    pub headline: Option<String>,
    /// What the headline was read from, before injected blocks were stripped. Shown as a tooltip
    /// so the operator can see the text the ledger cleaned up.
    pub headline_raw: Option<String>,
    /// Where the session ran, named the way the fleet sidebar names it (`repo/lane`). The daemon
    /// fills this in; a lane that has since been removed still reads as its repo.
    pub lane_label: Option<String>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub repo_id: Option<RepoId>,
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub lane_id: Option<LaneId>,
    pub cwd: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub turns: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub tool_calls: u32,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub retries: u32,
    pub totals: UsageTotals,
    pub estimated: bool,
    pub external: bool,
}

/// What kind of thing the optimize panel found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export, rename = "UsageFindingKind"))]
pub enum FindingKind {
    /// A model, repo or lane that accounts for a large share of the bill.
    CostDriver,
    /// Read-side tokens that mostly missed the prompt cache.
    CacheMiss,
    /// A session that spent many turns recovering from errors.
    Retries,
    /// Work that a cheaper model in the same family would likely have handled.
    ModelChoice,
}

/// One plain finding for the optimize panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageFinding {
    pub kind: FindingKind,
    /// What the finding is about: a model id, or a session id.
    pub subject: String,
    pub headline: String,
    pub detail: String,
    /// The dollars this finding concerns, so the panel can order by what is worth reading.
    pub cost_usd: f64,
    /// The session this finding points at, so the panel can link to its row. `None` when the
    /// finding is about a model, or folds several sessions into one line.
    pub session_id: Option<String>,
    /// How many sessions were folded into this line. One for a finding about a single session.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub count: u32,
}

/// What to call a session in a finding: its task headline, or the fact that it has none and where
/// it ran, so no line is ever addressed to a bare identifier.
pub fn session_name(row: &UsageSessionRow) -> String {
    match row.headline.as_deref().map(str::trim) {
        Some(headline) if !headline.is_empty() => headline.to_string(),
        _ => match row.lane_label.as_deref() {
            Some(lane) => format!("{UNTITLED_SESSION} in {lane}"),
            None => UNTITLED_SESSION.to_string(),
        },
    }
}

/// Below this cache hit rate a read-heavy workload is worth looking at.
const CACHE_MISS_FLOOR: f64 = 0.5;
/// Read-side tokens under this are too small for a cache finding to be worth reading.
const CACHE_MISS_MIN_TOKENS: u64 = 1_000_000;
/// A session with at least this share of retried turns is worth naming.
const RETRY_SHARE_FLOOR: f64 = 0.1;
/// Sessions shorter than this many turns are too small for a retry finding to mean anything.
const RETRY_MIN_TURNS: u32 = 10;
/// A premium-model session doing less work than this looks like it could have run cheaper.
const LIGHT_SESSION_OUTPUT_TOKENS: u64 = 5_000;

/// Plain findings for the optimize panel, ordered by the dollars each concerns.
pub fn findings(
    events: &[UsageEvent],
    sessions: &[UsageSessionRow],
    table: &PriceTable,
) -> Vec<UsageFinding> {
    let mut out = Vec::new();
    if !events.is_empty() {
        let by_model = summarize(events, GroupBy::Model, table);
        let total = by_model.totals.cost_usd;
        for row in by_model.groups.iter().take(3) {
            if row.totals.cost_usd <= 0.0 {
                continue;
            }
            let share = if total > 0.0 {
                row.totals.cost_usd / total * 100.0
            } else {
                0.0
            };
            out.push(UsageFinding {
                kind: FindingKind::CostDriver,
                subject: row.key.clone(),
                headline: format!("{} is {:.0} percent of the bill", row.key, share),
                detail: format!(
                    "{} turns, {} tokens, {}.",
                    row.totals.events,
                    tokens(row.totals.total_tokens),
                    money(row.totals.cost_usd)
                ),
                cost_usd: row.totals.cost_usd,
                session_id: None,
                count: 1,
            });
        }
        for row in by_model.groups.iter() {
            let read_side = row.totals.input_tokens + row.totals.cache_read_tokens;
            if read_side < CACHE_MISS_MIN_TOKENS {
                continue;
            }
            let hit = row.totals.cache_read_tokens as f64 / read_side as f64;
            if hit >= CACHE_MISS_FLOOR {
                continue;
            }
            out.push(UsageFinding {
                kind: FindingKind::CacheMiss,
                subject: row.key.clone(),
                headline: format!("{} reads at a {:.0} percent cache hit rate", row.key, hit * 100.0),
                detail: format!(
                    "{} uncached input tokens against {} cached. A stable prompt prefix is what moves this.",
                    tokens(row.totals.input_tokens),
                    tokens(row.totals.cache_read_tokens)
                ),
                cost_usd: row.totals.cost_usd,
                session_id: None,
                count: 1,
            });
        }
    }
    // Session findings fold by shape: every session that hit the same rule on the same model
    // becomes one line with a count and a total, rather than the same sentence twenty times.
    let mut retried: BTreeMap<&str, Vec<&UsageSessionRow>> = BTreeMap::new();
    let mut light: BTreeMap<&str, Vec<&UsageSessionRow>> = BTreeMap::new();
    for s in sessions {
        if s.turns >= RETRY_MIN_TURNS && s.retries as f64 / s.turns as f64 >= RETRY_SHARE_FLOOR {
            retried.entry(s.model.as_str()).or_default().push(s);
        }
        if cheaper_sibling(&s.model).is_some()
            && s.totals.output_tokens > 0
            && s.totals.output_tokens < LIGHT_SESSION_OUTPUT_TOKENS
            && s.tool_calls <= 2
        {
            light.entry(s.model.as_str()).or_default().push(s);
        }
    }
    for (model, rows) in retried {
        out.push(retry_finding(model, &rows));
    }
    for (model, rows) in light {
        out.push(model_choice_finding(model, &rows));
    }
    out.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd));
    out
}

/// The dearest session of a group, which is the one worth naming when a line folds several.
fn dearest<'a>(rows: &[&'a UsageSessionRow]) -> &'a UsageSessionRow {
    rows.iter()
        .copied()
        .max_by(|a, b| a.totals.cost_usd.total_cmp(&b.totals.cost_usd))
        .expect("a finding group is never empty")
}

fn group_cost(rows: &[&UsageSessionRow]) -> f64 {
    rows.iter().map(|r| r.totals.cost_usd).sum()
}

/// One line for the sessions that spent turns recovering from errors on `model`.
fn retry_finding(model: &str, rows: &[&UsageSessionRow]) -> UsageFinding {
    let cost = group_cost(rows);
    let retries: u32 = rows.iter().map(|r| r.retries).sum();
    let turns: u32 = rows.iter().map(|r| r.turns).sum();
    if let [only] = rows {
        return UsageFinding {
            kind: FindingKind::Retries,
            subject: only.session_id.clone(),
            headline: format!(
                "{} retried {} of {} turns",
                session_name(only),
                only.retries,
                only.turns
            ),
            detail: format!("{model}, {}.", money(cost)),
            cost_usd: cost,
            session_id: Some(only.session_id.clone()),
            count: 1,
        };
    }
    UsageFinding {
        kind: FindingKind::Retries,
        subject: model.to_string(),
        headline: format!(
            "{} sessions retried {retries} of {turns} turns on {model}",
            rows.len()
        ),
        detail: format!(
            "{} in total. The dearest is \"{}\".",
            money(cost),
            session_name(dearest(rows))
        ),
        cost_usd: cost,
        session_id: None,
        count: rows.len() as u32,
    }
}

/// One line for the sessions that did little work on a model with a cheaper sibling.
fn model_choice_finding(model: &str, rows: &[&UsageSessionRow]) -> UsageFinding {
    let cheaper = cheaper_sibling(model).unwrap_or(model);
    let cost = group_cost(rows);
    let output: u64 = rows.iter().map(|r| r.totals.output_tokens).sum();
    if let [only] = rows {
        return UsageFinding {
            kind: FindingKind::ModelChoice,
            subject: only.session_id.clone(),
            headline: format!("{} did light work on {model}", session_name(only)),
            detail: format!(
                "{} output tokens and {} tool calls. {cheaper} handles this shape of task.",
                tokens(only.totals.output_tokens),
                only.tool_calls
            ),
            cost_usd: cost,
            session_id: Some(only.session_id.clone()),
            count: 1,
        };
    }
    UsageFinding {
        kind: FindingKind::ModelChoice,
        subject: model.to_string(),
        headline: format!("{} sessions did light work on {model}", rows.len()),
        detail: format!(
            "{} output tokens and {} between them. {cheaper} handles this shape of task.",
            tokens(output),
            money(cost)
        ),
        cost_usd: cost,
        session_id: None,
        count: rows.len() as u32,
    }
}

/// The cheaper model in the same family, when there is an obvious one.
fn cheaper_sibling(model: &str) -> Option<&'static str> {
    if model.starts_with("claude-opus") || model.starts_with("claude-fable") {
        Some("claude-sonnet-5")
    } else if model.starts_with("claude-sonnet") {
        Some("claude-haiku-4-5")
    } else if model.starts_with("gemini-3-pro") {
        Some("gemini-3-flash")
    } else {
        None
    }
}

/// Format dollars for prose and tables: whole dollars above a thousand, cents below a hundred,
/// and a compact threshold for sub-cent amounts.
pub fn money(usd: f64) -> String {
    if usd == 0.0 {
        return "$0".to_string();
    }
    let sign = if usd < 0.0 { "-" } else { "" };
    let size = usd.abs();
    if size >= 1000.0 {
        return format!("{sign}${}", group_thousands(size.round() as u64));
    }
    if size >= 100.0 {
        return format!("{sign}${size:.1}");
    }
    if size >= 0.01 {
        return format!("{sign}${size:.2}");
    }
    format!("{sign}<$0.01")
}

/// Format a token count the way an axis label or a table cell wants it: k, M or B with one
/// decimal, and no trailing ".0" left behind.
pub fn tokens(count: u64) -> String {
    const SUFFIXES: [&str; 4] = ["", "k", "M", "B"];
    let mut value = count as f64;
    let mut unit = 0usize;
    // 999.95 rather than 1000: a value that would render as "1000.0k" belongs one unit up.
    while value >= 999.95 && unit + 1 < SUFFIXES.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        return count.to_string();
    }
    format!("{}{}", one_decimal(value), SUFFIXES[unit])
}

/// One decimal place, with a trailing ".0" dropped so "3.0k" reads "3k".
fn one_decimal(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
}

/// Digits in groups of three, so a four-figure sum is read at a glance.
fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The export header, in the order [`to_csv`] writes fields.
const CSV_HEADER: &str = "at,agent_kind,model,account,repo_id,lane_id,session_id,cwd,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,thinking_tokens,estimated,external,cost_usd";

/// Render events as CSV, one line per event, priced through `table`.
pub fn to_csv(events: &[UsageEvent], table: &PriceTable) -> String {
    let mut out = String::from(CSV_HEADER);
    out.push('\n');
    for e in events {
        let cost = table.cost(&e.model, e.at, &e.tokens()).unwrap_or(0.0);
        let fields = [
            e.at.to_rfc3339(),
            e.agent_kind.clone(),
            e.model.clone(),
            e.account.clone(),
            e.repo_id.map(|r| r.to_string()).unwrap_or_default(),
            e.lane_id.map(|l| l.to_string()).unwrap_or_default(),
            e.session_id.clone().unwrap_or_default(),
            e.cwd.clone().unwrap_or_default(),
            e.input_tokens.to_string(),
            e.output_tokens.to_string(),
            e.cache_read_tokens.to_string(),
            e.cache_write_tokens.to_string(),
            e.thinking_tokens.to_string(),
            e.estimated.to_string(),
            e.external.to_string(),
            format!("{cost:.6}"),
        ];
        let line: Vec<String> = fields.iter().map(|f| csv_field(f)).collect();
        out.push_str(&line.join(","));
        out.push('\n');
    }
    out
}

/// Quote a CSV field when it holds a comma, a quote or a newline.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The stored digest of one agent session, written beside its events.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSessionMeta {
    pub session_id: String,
    pub agent_kind: String,
    pub headline: Option<String>,
    /// The turn the headline was read from, before injected blocks were stripped.
    pub headline_raw: Option<String>,
    /// Which revision of [`scan::HEADLINE_VERSION`] computed `headline`. A row written by an
    /// older revision is stale even when its content still looks fine, and ingest re-digests it
    /// from `source_path` until this catches up.
    pub headline_version: u32,
    pub cwd: Option<String>,
    pub repo_id: Option<RepoId>,
    pub lane_id: Option<LaneId>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub turns: u32,
    pub tool_calls: u32,
    pub retries: u32,
    pub external: bool,
    pub source_path: Option<String>,
    /// Which revision of [`INGEST_VERSION`] counted `turns`, `tool_calls` and `retries`. A row
    /// below it has its counters replaced rather than added to on the next write.
    pub counts_version: u32,
}

/// How far one ingest source has been read, and what went wrong last time if anything did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageCursor {
    pub source_path: String,
    /// A byte offset for line sources, a millisecond epoch watermark for the OpenCode database.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub offset: u64,
    /// The source's modification time when it was last read, as a Unix second.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub mtime: i64,
    pub scanned_at: DateTime<Utc>,
    pub error: Option<String>,
    /// Which revision of the readers produced this source's stored events. A cursor below
    /// [`INGEST_VERSION`] is re-read from the start, its previous events replaced.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub ingest_version: u32,
}

/// The answer to `usage.status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts", ts(export))]
pub struct UsageStatus {
    /// How many sources the ledger tracks.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub sources: u64,
    /// The most recent scan across all sources.
    pub last_scan_at: Option<DateTime<Utc>>,
    /// Only the sources that failed, so a healthy ledger sends an empty list.
    pub errors: Vec<UsageCursor>,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub events: u64,
    pub first_event_at: Option<DateTime<Utc>>,
    pub last_event_at: Option<DateTime<Utc>>,
    /// Whether an ingest pass is running right now.
    pub ingesting: bool,
    /// Sources an older reader wrote that ingest has yet to re-read. Counts down to zero over a
    /// few passes after a reader is corrected.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub stale_sources: u64,
}

/// Prices each session’s aggregate tokens at its dominant model’s rate, approximating sessions that
/// switched models.
pub fn price_sessions(rows: &mut [UsageSessionRow], table: &PriceTable) {
    for row in rows.iter_mut() {
        let at = row.ended_at.or(row.started_at).unwrap_or_else(Utc::now);
        let tokens = TokenCounts {
            input: row.totals.input_tokens,
            output: row.totals.output_tokens,
            cache_read: row.totals.cache_read_tokens,
            cache_write: row.totals.cache_write_tokens,
        };
        row.totals.cost_usd = table.cost(&row.model, at, &tokens).unwrap_or(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pricing::PriceTable;
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, h, m, 0).unwrap()
    }

    fn index() -> FleetIndex {
        FleetIndex::new(
            vec![(7, PathBuf::from("/repos/demo"))],
            vec![
                (11, 7, PathBuf::from("/repos/demo")),
                (12, 7, PathBuf::from("/repos/demo/wt/feature")),
            ],
        )
    }

    #[test]
    fn attribution_picks_the_longest_matching_lane_worktree() {
        let a = index().attribute(Some("/repos/demo/wt/feature/crates/core"));
        assert_eq!(a.lane_id, Some(12));
        assert_eq!(a.repo_id, Some(7));
        assert!(!a.external);
    }

    #[test]
    fn a_cwd_inside_a_known_repo_but_no_lane_is_external() {
        let idx = FleetIndex::new(vec![(7, PathBuf::from("/repos/demo"))], vec![]);
        let a = idx.attribute(Some("/repos/demo/src"));
        assert_eq!(a.repo_id, Some(7));
        assert_eq!(a.lane_id, None);
        assert!(a.external, "repomon did not run this session");
    }

    #[test]
    fn an_unknown_cwd_attributes_to_nothing_and_is_external() {
        let a = index().attribute(Some("/elsewhere/scratch"));
        assert_eq!(a.repo_id, None);
        assert_eq!(a.lane_id, None);
        assert!(a.external);
        let none = index().attribute(None);
        assert!(none.external);
    }

    fn row(
        kind: &str,
        model: &str,
        hour: u32,
        minute: u32,
        input: u64,
        output: u64,
        cr: u64,
    ) -> UsageEvent {
        UsageEvent {
            at: at(hour, minute),
            agent_kind: kind.to_string(),
            model: model.to_string(),
            account: "default".to_string(),
            lane_id: Some(11),
            repo_id: Some(7),
            session_id: Some("s1".to_string()),
            window: None,
            cwd: Some("/repos/demo".to_string()),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cr,
            cache_write_tokens: 0,
            thinking_tokens: 0,
            estimated: false,
            external: false,
            subagent: false,
            source_path: "t.jsonl".to_string(),
            source_offset: 0,
        }
    }

    #[test]
    fn summary_totals_cache_hit_rate_and_estimated_share() {
        let mut estimated = row("antigravity", "gemini-3-flash", 2, 0, 100, 0, 0);
        estimated.estimated = true;
        let events = vec![
            row("claude-code", "claude-sonnet-5", 1, 0, 100, 50, 300),
            estimated,
        ];
        let s = summarize(&events, GroupBy::Kind, &PriceTable::builtin());
        assert_eq!(s.totals.input_tokens, 200);
        assert_eq!(s.totals.cache_read_tokens, 300);

        assert!((s.cache_hit_rate - 0.6).abs() < 1e-9);

        assert!((s.estimated_share - 100.0 / 550.0).abs() < 1e-9);
        assert_eq!(s.groups.len(), 2);
    }

    #[test]
    fn summary_groups_are_ordered_by_cost_descending() {
        let events = vec![
            row("claude-code", "claude-sonnet-5", 1, 0, 10, 10, 0),
            row("codex", "gpt-5.6-sol", 1, 0, 5_000_000, 5_000_000, 0),
        ];
        let s = summarize(&events, GroupBy::Kind, &PriceTable::builtin());
        assert_eq!(s.groups[0].key, "codex");
        assert!(s.groups[0].totals.cost_usd > s.groups[1].totals.cost_usd);
    }

    #[test]
    fn summary_can_group_by_model_repo_lane_and_account() {
        let events = vec![row("claude-code", "claude-sonnet-5", 1, 0, 10, 10, 0)];
        let table = PriceTable::builtin();
        assert_eq!(
            summarize(&events, GroupBy::Model, &table).groups[0].key,
            "claude-sonnet-5"
        );
        assert_eq!(summarize(&events, GroupBy::Repo, &table).groups[0].key, "7");
        assert_eq!(
            summarize(&events, GroupBy::Lane, &table).groups[0].key,
            "11"
        );
        assert_eq!(
            summarize(&events, GroupBy::Account, &table).groups[0].key,
            "default"
        );
    }

    #[test]
    fn a_priceless_model_still_counts_tokens_and_is_flagged_unpriced() {
        let events = vec![row("other", "some-local-model", 1, 0, 1_000_000, 0, 0)];
        let s = summarize(&events, GroupBy::Model, &PriceTable::builtin());
        assert_eq!(s.totals.cost_usd, 0.0);
        assert_eq!(s.totals.input_tokens, 1_000_000);
        assert!(s.unpriced_models.contains(&"some-local-model".to_string()));
        assert!(
            s.groups[0].unpriced,
            "the group itself should carry the flag, not just the summary-wide list"
        );
    }

    #[test]
    fn gpt_6_prices_off_the_placeholder_and_is_not_flagged_unpriced() {
        let mut e = row("codex", "gpt-6-astra", 1, 0, 1_000_000, 0, 0);
        e.at = Utc::now();
        let s = summarize(&[e], GroupBy::Model, &PriceTable::builtin());
        assert!(s.totals.cost_usd > 0.0);
        assert!(s.unpriced_models.is_empty());
        assert!(!s.groups[0].unpriced);
    }

    #[test]
    fn a_free_tier_model_prices_at_zero_without_the_unpriced_flag() {
        let events = vec![row(
            "opencode",
            "kimi-k2-thinking-free",
            1,
            0,
            1_000_000,
            0,
            0,
        )];
        let s = summarize(&events, GroupBy::Model, &PriceTable::builtin());
        assert_eq!(s.totals.cost_usd, 0.0);
        assert!(s.unpriced_models.is_empty());
        assert!(!s.groups[0].unpriced);
    }

    #[test]
    fn timeline_buckets_by_quarter_hour_hour_and_day() {
        let events = vec![
            row("claude-code", "claude-sonnet-5", 1, 5, 10, 0, 0),
            row("claude-code", "claude-sonnet-5", 1, 20, 10, 0, 0),
            row("claude-code", "claude-sonnet-5", 3, 0, 10, 0, 0),
        ];
        let table = PriceTable::builtin();
        let quarter = timeline(&events, Bucket::Quarter, GroupBy::Kind, &table);
        assert_eq!(quarter.series[0].points.len(), 3, "01:00, 01:15 and 03:00");
        let hour = timeline(&events, Bucket::Hour, GroupBy::Kind, &table);
        assert_eq!(hour.series[0].points.len(), 2);
        assert_eq!(hour.series[0].points[0].total_tokens, 20);
        let day = timeline(&events, Bucket::Day, GroupBy::Kind, &table);
        assert_eq!(day.series[0].points.len(), 1);
        assert_eq!(day.series[0].points[0].total_tokens, 30);
    }

    #[test]
    fn timeline_points_are_ordered_oldest_first() {
        let events = vec![
            row("claude-code", "claude-sonnet-5", 9, 0, 1, 0, 0),
            row("claude-code", "claude-sonnet-5", 2, 0, 1, 0, 0),
        ];
        let t = timeline(&events, Bucket::Hour, GroupBy::Kind, &PriceTable::builtin());
        assert!(t.series[0].points[0].at < t.series[0].points[1].at);
    }

    #[test]
    fn findings_name_the_top_cost_driver() {
        let events = vec![
            row(
                "claude-code",
                "claude-opus-5",
                1,
                0,
                4_000_000,
                4_000_000,
                0,
            ),
            row("codex", "gpt-5.6-sol", 1, 0, 10, 10, 0),
        ];
        let f = findings(&events, &[], &PriceTable::builtin());
        assert!(
            f.iter()
                .any(|x| x.kind == FindingKind::CostDriver && x.subject == "claude-opus-5")
        );
    }

    #[test]
    fn findings_flag_a_low_cache_hit_rate() {
        let events: Vec<UsageEvent> = (0..20)
            .map(|i| row("claude-code", "claude-sonnet-5", 1, i, 200_000, 1_000, 0))
            .collect();
        let f = findings(&events, &[], &PriceTable::builtin());
        assert!(f.iter().any(|x| x.kind == FindingKind::CacheMiss));
    }

    #[test]
    fn findings_flag_a_session_that_burned_retries() {
        let sessions = vec![UsageSessionRow {
            session_id: "s1".to_string(),
            agent_kind: "claude-code".to_string(),
            model: "claude-sonnet-5".to_string(),
            headline: Some("Fix the flake".to_string()),
            headline_raw: Some("Fix the flake".to_string()),
            lane_label: Some("demo/flake".to_string()),
            repo_id: Some(7),
            lane_id: Some(11),
            cwd: Some("/repos/demo".to_string()),
            started_at: Some(at(1, 0)),
            ended_at: Some(at(2, 0)),
            turns: 40,
            tool_calls: 12,
            retries: 9,
            totals: UsageTotals::default(),
            estimated: false,
            external: false,
        }];
        let f = findings(&[], &sessions, &PriceTable::builtin());
        assert!(
            f.iter()
                .any(|x| x.kind == FindingKind::Retries && x.subject == "s1")
        );
    }

    fn session(id: &str, model: &str, headline: Option<&str>, cost: f64) -> UsageSessionRow {
        UsageSessionRow {
            session_id: id.to_string(),
            agent_kind: "claude-code".to_string(),
            model: model.to_string(),
            headline: headline.map(str::to_string),
            headline_raw: headline.map(str::to_string),
            lane_label: Some("demo/flake".to_string()),
            repo_id: Some(7),
            lane_id: Some(11),
            cwd: Some("/repos/demo".to_string()),
            started_at: Some(at(1, 0)),
            ended_at: Some(at(2, 0)),
            turns: 40,
            tool_calls: 12,
            retries: 9,
            totals: UsageTotals {
                cost_usd: cost,
                ..Default::default()
            },
            estimated: false,
            external: false,
        }
    }

    #[test]
    fn a_finding_names_a_session_by_its_task_not_its_identifier() {
        let sessions = vec![session(
            "0f3a-uuid",
            "claude-sonnet-5",
            Some("Fix the flake"),
            1.0,
        )];
        let f = findings(&[], &sessions, &PriceTable::builtin());
        let retry = f
            .iter()
            .find(|x| x.kind == FindingKind::Retries)
            .expect("a retry finding");
        assert_eq!(retry.headline, "Fix the flake retried 9 of 40 turns");
        assert_eq!(retry.session_id.as_deref(), Some("0f3a-uuid"));
        assert_eq!(retry.count, 1);
    }

    #[test]
    fn a_session_with_no_task_text_is_named_by_its_lane() {
        let sessions = vec![session("0f3a-uuid", "claude-sonnet-5", None, 1.0)];
        let f = findings(&[], &sessions, &PriceTable::builtin());
        assert!(
            f[0].headline
                .starts_with("untitled session in demo/flake retried"),
            "got {:?}",
            f[0].headline
        );
    }

    #[test]
    fn findings_of_the_same_shape_fold_into_one_line_with_a_count_and_a_total() {
        let sessions = vec![
            session("a", "claude-sonnet-5", Some("One"), 1.0),
            session("b", "claude-sonnet-5", Some("Two"), 2.0),
            session("c", "claude-sonnet-5", Some("Three"), 3.0),
        ];
        let f = findings(&[], &sessions, &PriceTable::builtin());
        let retry = f
            .iter()
            .find(|x| x.kind == FindingKind::Retries)
            .expect("a retry finding");
        assert_eq!(
            retry.headline,
            "3 sessions retried 27 of 120 turns on claude-sonnet-5"
        );
        assert_eq!(retry.count, 3);
        assert_eq!(retry.cost_usd, 6.0);
        assert!(retry.session_id.is_none(), "a folded line links to nothing");
        assert!(retry.detail.contains("\"Three\""), "got {:?}", retry.detail);
    }

    #[test]
    fn money_is_whole_dollars_above_a_thousand_and_cents_below_a_hundred() {
        assert_eq!(money(0.0), "$0");
        assert_eq!(money(0.0042), "<$0.01");
        assert_eq!(money(0.0099), "<$0.01");
        assert_eq!(money(0.01), "$0.01");
        assert_eq!(money(0.42), "$0.42");
        assert_eq!(money(12.5), "$12.50");
        assert_eq!(money(523.45), "$523.5");
        assert_eq!(money(12_580.4), "$12,580");
        assert_eq!(money(1000.0), "$1,000");
    }

    #[test]
    fn token_counts_read_as_k_m_and_b_without_a_trailing_zero() {
        assert_eq!(tokens(0), "0");
        assert_eq!(tokens(999), "999");
        assert_eq!(tokens(1_000), "1k");
        assert_eq!(tokens(1_500), "1.5k");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(12_580_000_000), "12.6B");
    }

    #[test]
    fn a_free_tier_model_prices_at_zero_without_a_warning() {
        let events = vec![row("opencode", "kimi-k2-free", 1, 0, 1_000, 2_000, 0)];
        let s = summarize(&events, GroupBy::Model, &PriceTable::builtin());
        assert!(
            s.unpriced_models.is_empty(),
            "a free tier has a published rate, and it is zero"
        );
        assert_eq!(s.totals.cost_usd, 0.0);
        assert!(s.totals.total_tokens > 0, "its tokens still count");
    }

    #[test]
    fn findings_are_empty_for_an_empty_ledger() {
        assert!(findings(&[], &[], &PriceTable::builtin()).is_empty());
    }

    #[test]
    fn csv_export_has_a_header_and_one_line_per_event() {
        let events = vec![row("claude-code", "claude-sonnet-5", 1, 0, 10, 20, 30)];
        let csv = to_csv(&events, &PriceTable::builtin());
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].starts_with("at,agent_kind,model"));
        assert_eq!(lines.len(), 2);
        assert!(lines[1].contains("claude-sonnet-5"));
    }

    #[test]
    fn csv_quotes_a_field_that_holds_a_comma() {
        let mut e = row("claude-code", "claude-sonnet-5", 1, 0, 1, 1, 1);
        e.cwd = Some("/repos/a,b".to_string());
        let csv = to_csv(&[e], &PriceTable::builtin());
        assert!(csv.contains("\"/repos/a,b\""));
    }

    #[test]
    fn a_named_range_starts_at_local_midnight_and_week_covers_seven_days() {
        // The operator reads their own calendar, and so does the agent CLI they compare against,
        // so a day-aligned range starts where their day starts rather than at UTC midnight.
        use chrono::{Local, Timelike};
        let now = at(13, 30);
        let today_local = now.with_timezone(&Local).date_naive();
        let (from, to) = Range::Today.window(now);
        assert_eq!(to, now);
        let local = from.with_timezone(&Local);
        assert_eq!(local.date_naive(), today_local);
        assert_eq!((local.hour(), local.minute(), local.second()), (0, 0, 0));
        for (range, back) in [(Range::Week, 6), (Range::Month, 29)] {
            let (from, _) = range.window(now);
            let local = from.with_timezone(&Local);
            assert_eq!(local.date_naive(), today_local - Duration::days(back));
            assert_eq!(local.hour(), 0);
        }
    }
}
