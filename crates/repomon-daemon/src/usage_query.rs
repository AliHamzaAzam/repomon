//! The query side of the ledger: resolve a range, read the rows, price them, and put readable
//! labels on repo and lane groups before any of it reaches a client.
//!
//! These helpers exist so the RPC arms in [`crate::rpc`] stay one-liners and so the parts worth
//! testing are testable without a socket.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use repomon_core::usage_ledger::{
    Bucket, GroupBy, Range, UsageCursor, UsageFinding, UsageGroupRow, UsageSessionRow, UsageStatus,
    UsageSummary, UsageTimeline, findings as compute_findings, price_sessions, summarize,
    timeline as compute_timeline, to_csv,
};

use crate::Ctx;

/// Turn a range and its optional explicit bounds into a `[from, to]` window.
///
/// Explicit bounds always win, whatever the range is named. A day-aligned range means local
/// calendar days, and the client is the one that knows which zone the operator is reading in, so
/// a client that resolved its own preset sends the two instants and keeps the name only as a
/// label. A range with no bounds is resolved here instead, in the daemon's own zone.
///
/// A custom range with no bounds is treated as today rather than as all of history: a client that
/// forgot to send them gets a cheap answer, not the whole ledger.
pub fn resolve_window(
    range: Range,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    if let (Some(a), Some(b)) = (since, until) {
        return if a <= b { (a, b) } else { (b, a) };
    }
    if range == Range::Custom {
        if let Some(a) = since {
            return (a.min(now), now);
        }
    }
    let range = if range == Range::Custom {
        Range::Today
    } else {
        range
    };
    range.window(now)
}

/// Readable names for the ids a summary groups on.
#[derive(Debug, Clone, Default)]
pub struct Labels {
    repos: HashMap<i64, String>,
    lanes: HashMap<i64, String>,
}

impl Labels {
    /// Read repo and lane names from the store.
    pub async fn load(ctx: &Arc<Ctx>) -> Self {
        let repos = ctx.store.list_repos().await.unwrap_or_default();
        let lanes = ctx.store.list_lane_meta().await.unwrap_or_default();
        let repo_names: HashMap<i64, String> = repos
            .iter()
            .map(|r| (r.id, r.label.clone().unwrap_or_else(|| r.name.clone())))
            .collect();
        let lane_names = lanes
            .iter()
            .map(|l| {
                let repo = repo_names
                    .get(&l.repo_id)
                    .cloned()
                    .unwrap_or_else(|| l.repo_id.to_string());
                let leaf = l
                    .worktree_path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| l.id.to_string());
                (l.id, format!("{repo}/{leaf}"))
            })
            .collect();
        Labels {
            repos: repo_names,
            lanes: lane_names,
        }
    }

    /// Replace each row's label with a name, where the group is one that has names.
    pub fn apply(&self, group_by: GroupBy, rows: &mut [UsageGroupRow]) {
        for row in rows.iter_mut() {
            row.label = self.label_for(group_by, &row.key);
        }
    }

    /// Replace each series' label the same way.
    pub fn apply_series(&self, group_by: GroupBy, timeline: &mut UsageTimeline) {
        for s in timeline.series.iter_mut() {
            s.label = self.label_for(group_by, &s.key);
        }
    }

    /// Where a session ran, named the way the fleet sidebar names it. A lane the operator has
    /// since removed still reads as its repo, so the row says where the work happened rather than
    /// falling back to a bare path.
    pub fn session_label(&self, repo_id: Option<i64>, lane_id: Option<i64>) -> Option<String> {
        if let Some(id) = lane_id {
            if let Some(name) = self.lanes.get(&id) {
                return Some(name.clone());
            }
            return Some(match repo_id.and_then(|r| self.repos.get(&r)) {
                Some(repo) => format!("{repo} (lane removed)"),
                None => "lane removed".to_string(),
            });
        }
        repo_id.and_then(|r| self.repos.get(&r)).cloned()
    }

    fn label_for(&self, group_by: GroupBy, key: &str) -> String {
        if key.is_empty() {
            return "unattributed".to_string();
        }
        let id = key.parse::<i64>().ok();
        match (group_by, id) {
            (GroupBy::Repo, Some(id)) => self
                .repos
                .get(&id)
                .cloned()
                .unwrap_or_else(|| key.to_string()),
            (GroupBy::Lane, Some(id)) => self
                .lanes
                .get(&id)
                .cloned()
                .unwrap_or_else(|| key.to_string()),
            _ => key.to_string(),
        }
    }
}

/// A summary over `[from, to]`, priced and labelled.
pub async fn summary(
    ctx: &Arc<Ctx>,
    (from, to): (DateTime<Utc>, DateTime<Utc>),
    group_by: GroupBy,
) -> repomon_core::Result<UsageSummary> {
    let events = ctx.store.usage_events_between(from, to).await?;
    let table = crate::usage_ingest::price_table(ctx).await;
    let mut out = summarize(&events, group_by, &table);
    // The window is what was asked for, not what happened to have rows in it, so an empty day
    // still reads as that day.
    out.from = from;
    out.to = to;
    Labels::load(ctx).await.apply(group_by, &mut out.groups);
    // Feeds the price refresh's one ten-minute retry: a gap here after the day's fetch already
    // ran is worth one extra attempt (a newly-seen model, or a refresh that landed between
    // scans), but only once; see `usage_rates::RetryTracker`.
    crate::usage_rates::note_unpriced(ctx, !out.unpriced_models.is_empty()).await;
    Ok(out)
}

/// A timeline over `[from, to]`, priced and labelled.
pub async fn timeline(
    ctx: &Arc<Ctx>,
    (from, to): (DateTime<Utc>, DateTime<Utc>),
    bucket: Bucket,
    group_by: GroupBy,
) -> repomon_core::Result<UsageTimeline> {
    let events = ctx.store.usage_events_between(from, to).await?;
    let table = crate::usage_ingest::price_table(ctx).await;
    let mut out = compute_timeline(&events, bucket, group_by, &table);
    Labels::load(ctx).await.apply_series(group_by, &mut out);
    Ok(out)
}

/// Session rows over `[from, to]`, priced.
pub async fn sessions(
    ctx: &Arc<Ctx>,
    (from, to): (DateTime<Utc>, DateTime<Utc>),
    lane_id: Option<i64>,
    limit: usize,
) -> repomon_core::Result<Vec<UsageSessionRow>> {
    let mut rows = ctx
        .store
        .usage_sessions_between(from, to, lane_id, limit.clamp(1, 500))
        .await?;
    let table = crate::usage_ingest::price_table(ctx).await;
    price_sessions(&mut rows, &table);
    let labels = Labels::load(ctx).await;
    for row in rows.iter_mut() {
        row.lane_label = labels.session_label(row.repo_id, row.lane_id);
    }
    Ok(rows)
}

/// Optimize-panel findings over `[from, to]`.
pub async fn findings(
    ctx: &Arc<Ctx>,
    window: (DateTime<Utc>, DateTime<Utc>),
) -> repomon_core::Result<Vec<UsageFinding>> {
    let events = ctx.store.usage_events_between(window.0, window.1).await?;
    let rows = sessions(ctx, window, None, 200).await?;
    let table = crate::usage_ingest::price_table(ctx).await;
    Ok(compute_findings(&events, &rows, &table))
}

/// What the ledger knows about its own health.
pub async fn status(ctx: &Arc<Ctx>) -> repomon_core::Result<UsageStatus> {
    let cursors: Vec<UsageCursor> = ctx.store.usage_cursors().await?;
    let (events, first, last) = ctx.store.usage_extent().await?;
    let stale_sources = ctx
        .store
        .usage_sources_below_ingest_version(repomon_core::usage_ledger::INGEST_VERSION)
        .await?;
    Ok(UsageStatus {
        sources: cursors.len() as u64,
        last_scan_at: cursors.iter().map(|c| c.scanned_at).max(),
        errors: cursors.into_iter().filter(|c| c.error.is_some()).collect(),
        events,
        first_event_at: first,
        last_event_at: last,
        ingesting: ctx.usage_ingest_lock.try_lock().is_err(),
        stale_sources,
    })
}

/// The Settings > Usage "Model rates" table: one row per model the ledger has ever seen, plus one
/// for every model with a `[usage.price_overrides]` entry the ledger hasn't seen yet.
pub async fn model_rates(ctx: &Arc<Ctx>) -> repomon_core::Result<Vec<repomon_core::pricing::ModelRateRow>> {
    let table = crate::usage_ingest::price_table(ctx).await;
    let overrides = ctx.config.read().await.usage.price_overrides.clone();
    let now = Utc::now();
    let seen = ctx
        .store
        .usage_model_seen(now - chrono::Duration::days(30))
        .await?;
    Ok(repomon_core::pricing::model_rate_rows(
        &table, &overrides, &seen, now,
    ))
}

/// Which file an export writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    #[default]
    Csv,
    Json,
}

/// Where an export landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageExport {
    pub path: String,
    pub events: u64,
    pub bytes: u64,
}

/// Write the events in `[from, to]` to a file under the data directory and say where it went.
///
/// The daemon writes the file rather than returning its bytes: an export of a busy month is
/// megabytes, and a path is what the operator wants to do something with anyway.
pub async fn export(
    ctx: &Arc<Ctx>,
    (from, to): (DateTime<Utc>, DateTime<Utc>),
    format: ExportFormat,
) -> repomon_core::Result<UsageExport> {
    let events = ctx.store.usage_events_between(from, to).await?;
    let table = crate::usage_ingest::price_table(ctx).await;
    let body = match format {
        ExportFormat::Csv => to_csv(&events, &table),
        ExportFormat::Json => {
            let rows: Vec<serde_json::Value> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "at": e.at,
                        "agent_kind": e.agent_kind,
                        "model": e.model,
                        "account": e.account,
                        "repo_id": e.repo_id,
                        "lane_id": e.lane_id,
                        "session_id": e.session_id,
                        "window": e.window,
                        "cwd": e.cwd,
                        "input_tokens": e.input_tokens,
                        "output_tokens": e.output_tokens,
                        "cache_read_tokens": e.cache_read_tokens,
                        "cache_write_tokens": e.cache_write_tokens,
                        "thinking_tokens": e.thinking_tokens,
                        "estimated": e.estimated,
                        "external": e.external,
                        "cost_usd": table.cost(&e.model, e.at, &e.tokens()).unwrap_or(0.0),
                    })
                })
                .collect();
            serde_json::to_string_pretty(&rows)?
        }
    };
    let ext = match format {
        ExportFormat::Csv => "csv",
        ExportFormat::Json => "json",
    };
    let dir = repomon_core::config::data_dir().join("exports");
    let name = format!(
        "usage-{}-{}.{ext}",
        from.format("%Y%m%dT%H%M%S"),
        to.format("%Y%m%dT%H%M%S")
    );
    let path = dir.join(name);
    let events_count = events.len() as u64;
    let written = tokio::task::spawn_blocking(move || -> std::io::Result<(String, u64)> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, body.as_bytes())?;
        Ok((path.to_string_lossy().to_string(), body.len() as u64))
    })
    .await
    .map_err(|e| repomon_core::Error::Other(e.to_string()))??;
    Ok(UsageExport {
        path: written.0,
        events: events_count,
        bytes: written.1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use repomon_core::{Config, Store};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 5, 13, 30, 0).unwrap()
    }

    #[test]
    fn a_named_range_with_no_bounds_resolves_to_local_calendar_days() {
        use chrono::{Local, Timelike};
        let (from, to) = resolve_window(Range::Week, None, None, now());
        assert_eq!(to, now());
        let local = from.with_timezone(&Local);
        assert_eq!(local.hour(), 0);
        assert_eq!(
            local.date_naive(),
            now().with_timezone(&Local).date_naive() - chrono::Duration::days(6)
        );
    }

    #[test]
    fn a_named_range_takes_the_bounds_a_client_resolved_for_itself() {
        // The desktop resolves its presets in the browser's zone and sends them outright, so a
        // client in another zone than the daemon still reads the window it drew.
        let since = Utc.with_ymd_and_hms(2026, 8, 29, 19, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2026, 9, 5, 13, 30, 0).unwrap();
        let (from, to) = resolve_window(Range::Week, Some(since), Some(until), now());
        assert_eq!(from, since);
        assert_eq!(to, until);
    }

    #[test]
    fn a_custom_range_uses_the_bounds_it_was_given() {
        let since = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let until = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let (from, to) = resolve_window(Range::Custom, Some(since), Some(until), now());
        assert_eq!(from, since);
        assert_eq!(to, until);
    }

    #[test]
    fn a_custom_range_missing_its_bounds_falls_back_to_today() {
        use chrono::{Local, Timelike};
        let (from, to) = resolve_window(Range::Custom, None, None, now());
        let local = from.with_timezone(&Local);
        assert_eq!(local.date_naive(), now().with_timezone(&Local).date_naive());
        assert_eq!(local.hour(), 0);
        assert_eq!(to, now());
    }

    #[test]
    fn a_backwards_custom_range_is_swapped_rather_than_returning_nothing() {
        let a = Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let (from, to) = resolve_window(Range::Custom, Some(a), Some(b), now());
        assert_eq!(from, b);
        assert_eq!(to, a);
    }

    async fn ctx_with_repo() -> (Arc<Ctx>, i64, i64) {
        let store = Store::open_in_memory().unwrap();
        let repo = store
            .add_repo("/repos/demo".into(), "demo".to_string(), None)
            .await
            .unwrap();
        let lane = store
            .get_or_create_lane(repo.id, "/repos/demo/wt/feature".to_string())
            .await
            .unwrap();
        (Ctx::new(store, Config::default(), None), repo.id, lane)
    }

    #[tokio::test]
    async fn repo_and_lane_groups_are_labelled_with_names_not_row_ids() {
        let (ctx, repo_id, lane_id) = ctx_with_repo().await;
        let labels = Labels::load(&ctx).await;
        let mut rows = [
            UsageGroupRow {
                key: repo_id.to_string(),
                label: repo_id.to_string(),
                totals: Default::default(),
                unpriced: false,
            },
            UsageGroupRow {
                key: lane_id.to_string(),
                label: lane_id.to_string(),
                totals: Default::default(),
                unpriced: false,
            },
        ];
        labels.apply(GroupBy::Repo, &mut rows[..1]);
        labels.apply(GroupBy::Lane, &mut rows[1..]);
        assert_eq!(rows[0].label, "demo");
        assert_eq!(rows[1].label, "demo/feature");
    }

    #[tokio::test]
    async fn a_session_lane_reads_as_repo_slash_lane_and_says_when_the_lane_is_gone() {
        let (ctx, repo_id, lane_id) = ctx_with_repo().await;
        let labels = Labels::load(&ctx).await;
        assert_eq!(
            labels.session_label(Some(repo_id), Some(lane_id)).as_deref(),
            Some("demo/feature")
        );
        assert_eq!(
            labels.session_label(Some(repo_id), Some(9_999)).as_deref(),
            Some("demo (lane removed)")
        );
        assert_eq!(
            labels.session_label(Some(repo_id), None).as_deref(),
            Some("demo")
        );
        assert_eq!(labels.session_label(None, None), None);
    }

    #[tokio::test]
    async fn an_unknown_group_key_keeps_a_readable_label() {
        let (ctx, _, _) = ctx_with_repo().await;
        let labels = Labels::load(&ctx).await;
        let mut rows = [UsageGroupRow {
            key: String::new(),
            label: String::new(),
            totals: Default::default(),
            unpriced: false,
        }];
        labels.apply(GroupBy::Repo, &mut rows);
        assert_eq!(rows[0].label, "unattributed");
    }

    #[tokio::test]
    async fn an_unpriced_model_is_flagged_on_its_group_through_the_daemon_query() {
        let (ctx, _, _) = ctx_with_repo().await;
        let event = repomon_core::usage_ledger::UsageEvent {
            at: Utc::now(),
            agent_kind: "codex".to_string(),
            model: "totally-unpublished-model".to_string(),
            account: "codex".to_string(),
            lane_id: None,
            repo_id: None,
            session_id: Some("s1".to_string()),
            window: None,
            cwd: None,
            input_tokens: 1_000,
            output_tokens: 500,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            thinking_tokens: 0,
            estimated: false,
            subagent: false,
            external: true,
            source_path: "t.jsonl".to_string(),
            source_offset: 0,
        };
        ctx.store
            .record_usage_events(vec![event.clone()])
            .await
            .unwrap();
        let out = summary(
            &ctx,
            (event.at - chrono::Duration::minutes(1), event.at + chrono::Duration::minutes(1)),
            GroupBy::Model,
        )
        .await
        .unwrap();
        assert!(out.unpriced_models.contains(&"totally-unpublished-model".to_string()));
        let row = out
            .groups
            .iter()
            .find(|g| g.key == "totally-unpublished-model")
            .expect("the model's own group should be present");
        assert!(row.unpriced, "the group itself should carry the flag");
    }

    #[tokio::test]
    async fn an_export_writes_a_file_under_the_data_directory_and_names_it() {
        // `REPOMON_DATA_DIR` is not one of the source-location variables the ingest tests move,
        // so this needs no lock against them.
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("REPOMON_DATA_DIR").ok();
        unsafe { std::env::set_var("REPOMON_DATA_DIR", dir.path()) };
        let (ctx, _, _) = ctx_with_repo().await;
        let out = export(
            &ctx,
            resolve_window(Range::Today, None, None, Utc::now()),
            ExportFormat::Csv,
        )
        .await
        .unwrap();
        assert!(out.path.starts_with(dir.path().to_string_lossy().as_ref()));
        assert!(
            std::fs::read_to_string(&out.path)
                .unwrap()
                .starts_with("at,agent_kind")
        );
        match prev {
            Some(v) => unsafe { std::env::set_var("REPOMON_DATA_DIR", v) },
            None => unsafe { std::env::remove_var("REPOMON_DATA_DIR") },
        }
    }

    #[tokio::test]
    async fn status_reports_an_empty_ledger_without_errors() {
        let (ctx, _, _) = ctx_with_repo().await;
        let s = status(&ctx).await.unwrap();
        assert_eq!(s.events, 0);
        assert_eq!(s.sources, 0);
        assert!(s.errors.is_empty());
        assert!(!s.ingesting);
    }

    #[tokio::test]
    async fn model_rates_lists_a_seen_model_and_an_unseen_overridden_one() {
        let (ctx, _, _) = ctx_with_repo().await;
        ctx.config.write().await.usage.refresh_prices = false;
        ctx.store
            .record_usage_events(vec![repomon_core::usage_ledger::UsageEvent {
                at: Utc::now(),
                agent_kind: "claude-code".to_string(),
                model: "claude-sonnet-5".to_string(),
                account: "default".to_string(),
                lane_id: None,
                repo_id: None,
                session_id: Some("s1".to_string()),
                window: None,
                cwd: None,
                input_tokens: 1_000,
                output_tokens: 500,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                thinking_tokens: 0,
                estimated: false,
                subagent: false,
                external: true,
                source_path: "t.jsonl".to_string(),
                source_offset: 0,
            }])
            .await
            .unwrap();
        ctx.config.write().await.usage.price_overrides.insert(
            "gpt-7".to_string(),
            repomon_core::pricing::PriceOverride {
                input_per_mtok: Some(4.0),
                ..Default::default()
            },
        );

        let rows = model_rates(&ctx).await.unwrap();
        let sonnet = rows.iter().find(|r| r.model == "claude-sonnet-5").unwrap();
        assert!(sonnet.last_seen.is_some());
        assert_eq!(sonnet.tokens_30d, 1_500);
        let gpt7 = rows.iter().find(|r| r.model == "gpt-7").unwrap();
        assert_eq!(gpt7.source, repomon_core::pricing::ModelRateSource::Override);
        assert_eq!(gpt7.last_seen, None, "never seen in the ledger");


    }
}
