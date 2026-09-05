//! The usage ledger: token counts per agent turn, attributed to the fleet and priced at query
//! time.
//!
//! The ledger stores tokens, never dollars. Every query re-prices its rows through a
//! [`crate::pricing::PriceTable`], so correcting a rate corrects history too, and a model with no
//! published price still contributes its tokens and is named in `unpriced_models` rather than
//! silently costing nothing.
//!
//! Everything in this module is a pure function over rows. Reading the sources lives in [`scan`],
//! persistence lives in [`crate::store`], and scheduling lives in the daemon.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{LaneId, RepoId};
use crate::pricing::{PriceTable, TokenCounts};

pub mod scan;

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

/// One row of the daily rollup table.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageDailyRow {
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    pub agent_kind: String,
    pub model: String,
    pub account: String,
    pub repo_id: Option<RepoId>,
    pub lane_id: Option<LaneId>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub thinking_tokens: u64,
    /// Of the tokens in this row, how many were estimated rather than reported.
    pub estimated_tokens: u64,
    pub events: u64,
}

/// The UTC day an instant falls in, `YYYY-MM-DD`.
pub fn day_key(at: DateTime<Utc>) -> String {
    at.format("%Y-%m-%d").to_string()
}

/// Fold events into daily rows, one per day, kind, model, account, repo and lane.
pub fn rollup(events: &[UsageEvent]) -> Vec<UsageDailyRow> {
    type Key = (
        String,
        String,
        String,
        String,
        Option<RepoId>,
        Option<LaneId>,
    );
    let mut by: BTreeMap<Key, UsageDailyRow> = BTreeMap::new();
    for e in events {
        let key = (
            day_key(e.at),
            e.agent_kind.clone(),
            e.model.clone(),
            e.account.clone(),
            e.repo_id,
            e.lane_id,
        );
        let row = by.entry(key.clone()).or_insert_with(|| UsageDailyRow {
            day: key.0.clone(),
            agent_kind: key.1.clone(),
            model: key.2.clone(),
            account: key.3.clone(),
            repo_id: key.4,
            lane_id: key.5,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            thinking_tokens: 0,
            estimated_tokens: 0,
            events: 0,
        });
        row.input_tokens += e.input_tokens;
        row.output_tokens += e.output_tokens;
        row.cache_read_tokens += e.cache_read_tokens;
        row.cache_write_tokens += e.cache_write_tokens;
        row.thinking_tokens += e.thinking_tokens;
        if e.estimated {
            row.estimated_tokens += e.total_tokens();
        }
        row.events += 1;
    }
    by.into_values().collect()
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
    /// The `[from, to]` window this range covers as of `now`. Day-aligned ranges start at UTC
    /// midnight so a day's total does not shift as the clock moves.
    pub fn window(self, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
        let midnight = Utc
            .with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
            .single()
            .unwrap_or(now);
        let back = match self {
            Range::Today => 0,
            Range::Week => 6,
            Range::Month => 29,
            Range::Custom => 0,
        };
        (midnight - Duration::days(back), now)
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
    let mut unpriced: BTreeMap<String, ()> = BTreeMap::new();
    for e in events {
        totals.add(e, table);
        groups.entry(group_by.key_of(e)).or_default().add(e, table);
        if table.lookup(&e.model, e.at).is_none() && !e.model.is_empty() {
            unpriced.insert(e.model.clone(), ());
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
        .map(|(key, totals)| UsageGroupRow {
            label: key.clone(),
            key,
            totals,
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
                    row.totals.total_tokens,
                    money(row.totals.cost_usd)
                ),
                cost_usd: row.totals.cost_usd,
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
                    row.totals.input_tokens, row.totals.cache_read_tokens
                ),
                cost_usd: row.totals.cost_usd,
            });
        }
    }
    for s in sessions {
        if s.turns >= RETRY_MIN_TURNS {
            let share = s.retries as f64 / s.turns as f64;
            if share >= RETRY_SHARE_FLOOR {
                out.push(UsageFinding {
                    kind: FindingKind::Retries,
                    subject: s.session_id.clone(),
                    headline: format!("{} of {} turns were retries", s.retries, s.turns),
                    detail: s
                        .headline
                        .clone()
                        .unwrap_or_else(|| "This session retried often.".to_string()),
                    cost_usd: s.totals.cost_usd,
                });
            }
        }
        if let Some(cheaper) = cheaper_sibling(&s.model) {
            if s.totals.output_tokens > 0
                && s.totals.output_tokens < LIGHT_SESSION_OUTPUT_TOKENS
                && s.tool_calls <= 2
            {
                out.push(UsageFinding {
                    kind: FindingKind::ModelChoice,
                    subject: s.session_id.clone(),
                    headline: format!("{} did light work on {}", s.session_id, s.model),
                    detail: format!(
                        "{} output tokens and {} tool calls. {cheaper} handles this shape of task.",
                        s.totals.output_tokens, s.tool_calls
                    ),
                    cost_usd: s.totals.cost_usd,
                });
            }
        }
    }
    out.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd));
    out
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

/// Format dollars for prose, with enough places that a cent-scale figure is still readable.
pub fn money(usd: f64) -> String {
    if usd >= 1.0 {
        format!("${usd:.2}")
    } else {
        format!("${usd:.4}")
    }
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
}

/// Price session rows through `table`, charging each row's totals at its dominant model's rate.
///
/// A session that switched models mid-run is priced as if it had stayed on the model it spent the
/// most tokens on. Splitting the cost exactly would mean keeping per-model totals per session,
/// which is what `usage.summary` grouped by model already answers.
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
            source_path: "t.jsonl".to_string(),
            source_offset: 0,
        }
    }

    #[test]
    fn rollup_sums_one_day_per_kind_model_repo_lane_and_account() {
        let rows = rollup(&[
            row("claude-code", "claude-sonnet-5", 1, 0, 10, 20, 30),
            row("claude-code", "claude-sonnet-5", 5, 0, 1, 2, 3),
            row("codex", "gpt-5.6-sol", 5, 0, 100, 200, 0),
        ]);
        assert_eq!(rows.len(), 2, "the two same-model same-day rows merge");
        let claude = rows.iter().find(|r| r.agent_kind == "claude-code").unwrap();
        assert_eq!(claude.day, "2026-09-01");
        assert_eq!(claude.input_tokens, 11);
        assert_eq!(claude.output_tokens, 22);
        assert_eq!(claude.cache_read_tokens, 33);
        assert_eq!(claude.events, 2);
    }

    #[test]
    fn rollup_keeps_days_apart() {
        let mut late = row("claude-code", "claude-sonnet-5", 1, 0, 5, 5, 0);
        late.at = Utc.with_ymd_and_hms(2026, 9, 2, 1, 0, 0).unwrap();
        let rows = rollup(&[row("claude-code", "claude-sonnet-5", 1, 0, 5, 5, 0), late]);
        assert_eq!(rows.len(), 2);
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
        // 300 cached against 200 uncached input across both rows.
        assert!((s.cache_hit_rate - 0.6).abs() < 1e-9);
        // 100 of 550 counted tokens came from an estimate.
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
    fn range_today_starts_at_utc_midnight_and_week_covers_seven_days() {
        let now = at(13, 30);
        let (from, to) = Range::Today.window(now);
        assert_eq!(from, Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap());
        assert_eq!(to, now);
        let (from, _) = Range::Week.window(now);
        assert_eq!(from, Utc.with_ymd_and_hms(2026, 8, 26, 0, 0, 0).unwrap());
        let (from, _) = Range::Month.window(now);
        assert_eq!(from, Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).unwrap());
    }
}
