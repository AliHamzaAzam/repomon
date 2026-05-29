//! JSON-RPC method dispatch.

use std::path::PathBuf;

use repomon_core::git::reader;
use repomon_core::model::{Commit, CreateLaneParams, RepoId, TimeRange};
use repomon_core::protocol::RpcError;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::Ctx;

fn internal<E: std::fmt::Display>(e: E) -> RpcError {
    RpcError::internal(e.to_string())
}

fn parse<T: DeserializeOwned>(params: Option<Value>) -> Result<T, RpcError> {
    serde_json::from_value(params.unwrap_or(Value::Null))
        .map_err(|e| RpcError::invalid_params(e.to_string()))
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

#[derive(Deserialize)]
struct RepoAdd {
    path: String,
}
#[derive(Deserialize)]
struct RepoRemove {
    repo_id: RepoId,
}
#[derive(Deserialize)]
struct Discover {
    root: String,
    #[serde(default = "default_depth")]
    max_depth: usize,
}
fn default_depth() -> usize {
    4
}
#[derive(Deserialize)]
struct LaneId {
    lane_id: repomon_core::model::LaneId,
}
#[derive(Deserialize)]
struct LaneDelete {
    lane_id: repomon_core::model::LaneId,
    #[serde(default)]
    also_delete_branch: bool,
}
#[derive(Deserialize)]
struct CommitRange {
    from_iso: String,
    to_iso: String,
    #[serde(default)]
    repo_ids: Option<Vec<RepoId>>,
}

/// Dispatch a single request to its handler.
pub async fn dispatch(ctx: &Ctx, method: &str, params: Option<Value>) -> Result<Value, RpcError> {
    match method {
        // ---- repos ----
        "repo.list" => to_value(ctx.registry.list().await.map_err(internal)?),
        "repo.add" => {
            let p: RepoAdd = parse(params)?;
            let repo = ctx
                .registry
                .add(std::path::Path::new(&p.path))
                .await
                .map_err(internal)?;
            ctx.broadcast(crate::pubsub::topic::REPO_ADDED, json!({ "repo": repo }));
            to_value(repo)
        }
        "repo.remove" => {
            let p: RepoRemove = parse(params)?;
            ctx.registry.remove(p.repo_id).await.map_err(internal)?;
            ctx.broadcast(
                crate::pubsub::topic::REPO_REMOVED,
                json!({ "repo_id": p.repo_id }),
            );
            Ok(Value::Null)
        }
        "repo.discover" => {
            let p: Discover = parse(params)?;
            let found = ctx
                .registry
                .discover(std::path::Path::new(&p.root), p.max_depth)
                .await
                .map_err(internal)?;
            let paths: Vec<String> = found
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            to_value(paths)
        }

        // ---- lanes ----
        "lane.list" => to_value(ctx.lanes.list().await.map_err(internal)?),
        "lane.get" => {
            let p: LaneId = parse(params)?;
            to_value(ctx.lanes.get(p.lane_id).await.map_err(internal)?)
        }
        "lane.create" => {
            let p: CreateLaneParams = parse(params)?;
            let lane = ctx.lanes.create(p).await.map_err(internal)?;
            ctx.broadcast(crate::pubsub::topic::LANE_CREATED, json!({ "lane": lane }));
            to_value(lane)
        }
        "lane.delete" => {
            let p: LaneDelete = parse(params)?;
            ctx.lanes
                .delete(p.lane_id, p.also_delete_branch)
                .await
                .map_err(internal)?;
            ctx.broadcast(
                crate::pubsub::topic::LANE_DELETED,
                json!({ "lane_id": p.lane_id }),
            );
            Ok(Value::Null)
        }
        "lane.focus" => {
            let p: LaneId = parse(params)?;
            let path = ctx.lanes.focus(p.lane_id).await.map_err(internal)?;
            Ok(json!({ "path": path.to_string_lossy() }))
        }

        // ---- commits (computed live via gix) ----
        "commit.today" => {
            let range = today_range();
            to_value(commits_in_range(ctx, range, None).await?)
        }
        "commit.range" => {
            let p: CommitRange = parse(params)?;
            let from = parse_iso(&p.from_iso)?;
            let to = parse_iso(&p.to_iso)?;
            to_value(commits_in_range(ctx, TimeRange { from, to }, p.repo_ids).await?)
        }

        // ---- subscription is handled in the socket layer ----
        "subscribe" => Ok(Value::Null),
        "viewport.set" => Ok(Value::Null), // wired up in M9

        // ---- daemon ----
        "daemon.status" => {
            let repos = ctx.registry.list().await.map_err(internal)?.len();
            let lanes = ctx.lanes.list().await.map_err(internal)?.len();
            let db_size = ctx
                .db_path
                .as_ref()
                .and_then(|p| std::fs::metadata(p).ok())
                .map(|m| m.len())
                .unwrap_or(0);
            Ok(json!({
                "uptime_secs": ctx.started.elapsed().as_secs(),
                "repos": repos,
                "lanes": lanes,
                "db_size_bytes": db_size,
                "version": repomon_core::version(),
            }))
        }
        "daemon.shutdown" => {
            ctx.request_shutdown();
            Ok(Value::Null)
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

fn parse_iso(s: &str) -> Result<chrono::DateTime<chrono::Utc>, RpcError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|e| RpcError::invalid_params(format!("bad timestamp {s:?}: {e}")))
}

/// The current local day, in UTC: [local midnight, next local midnight). Using the next
/// midnight as the exclusive end (rather than `now`) avoids dropping a commit made in the
/// same whole second as the query.
fn today_range() -> TimeRange {
    use chrono::{Local, TimeZone, Utc};
    let now_local = Local::now();
    let midnight_naive = now_local.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let from = Local
        .from_local_datetime(&midnight_naive)
        .single()
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);
    TimeRange {
        from,
        to: from + chrono::Duration::days(1),
    }
}

/// Aggregate commits across all (or selected) repos for `range`, newest first.
async fn commits_in_range(
    ctx: &Ctx,
    range: TimeRange,
    repo_ids: Option<Vec<RepoId>>,
) -> Result<Vec<Commit>, RpcError> {
    let repos = ctx.registry.list().await.map_err(internal)?;
    let mut out: Vec<Commit> = Vec::new();
    for repo in repos {
        if let Some(ids) = &repo_ids {
            if !ids.contains(&repo.id) {
                continue;
            }
        }
        let path: PathBuf = repo.path.clone();
        let id = repo.id;
        let commits =
            tokio::task::spawn_blocking(move || reader::read_commits_in_range(&path, id, range))
                .await
                .map_err(internal)?
                .unwrap_or_default();
        out.extend(commits);
    }
    out.sort_by_key(|c| std::cmp::Reverse(c.time));
    Ok(out)
}
