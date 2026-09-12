//! Pull-request strips for the home screen: `gh pr list` per tracked repo, ready-for-review
//! first. Only runs when `gh` is on PATH; a missing binary or a `gh` failure yields an empty
//! list rather than an error card, per the home-screen contract.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use repomon_core::model::PullRequestSummary;
use serde::Deserialize;

use crate::Ctx;

#[derive(Debug, Deserialize)]
struct GhPr {
    number: u32,
    title: String,
    url: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "updatedAt")]
    updated_at: DateTime<Utc>,
}

/// Every open PR across tracked, non-hidden repos, ready-for-review first and newest within
/// that. Empty when `gh` is not on PATH.
pub async fn list(ctx: &Arc<Ctx>) -> Vec<PullRequestSummary> {
    if repomon_core::exec::find_in_path("gh").is_none() {
        return Vec::new();
    }
    let repos = ctx.store.list_repos().await.unwrap_or_default();
    let mut out = Vec::new();
    for repo in repos.into_iter().filter(|r| !r.hidden) {
        let path = repo.path.clone();
        let prs = tokio::task::spawn_blocking(move || run_gh_pr_list(&path))
            .await
            .unwrap_or_default();
        let repo_id = repo.id;
        let repo_name = repo.label.clone().unwrap_or_else(|| repo.name.clone());
        out.extend(prs.into_iter().map(|pr| PullRequestSummary {
            repo_id,
            repo_name: repo_name.clone(),
            number: pr.number,
            title: pr.title,
            url: pr.url,
            is_draft: pr.is_draft,
            updated_at: pr.updated_at,
        }));
    }
    out.sort_by(|a, b| {
        a.is_draft
            .cmp(&b.is_draft)
            .then(b.updated_at.cmp(&a.updated_at))
    });
    out
}

fn run_gh_pr_list(repo_path: &Path) -> Vec<GhPr> {
    let Ok(output) = Command::new("gh")
        .args(["pr", "list", "--json", "number,title,url,isDraft,updatedAt"])
        .current_dir(repo_path)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    serde_json::from_slice(&output.stdout).unwrap_or_default()
}

/// Refresh the cache off the request path. A cache miss must never put GitHub network I/O in
/// front of whatever the operator is typing on the same connection; the caller serves what it
/// has and the next call picks up the result. One refresh at a time.
pub fn refresh_in_background(ctx: Arc<Ctx>) {
    tokio::spawn(async move {
        let Ok(_guard) = ctx.pr_refresh.try_lock() else {
            return;
        };
        let items = list(&ctx).await;
        *ctx.pr_cache.lock().await = Some((std::time::Instant::now(), items));
    });
}
