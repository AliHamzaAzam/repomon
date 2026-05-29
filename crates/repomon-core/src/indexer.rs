//! Full-history commit indexer (Phase 3).
//!
//! Walks each repo's HEAD ancestry and stores commits in SQLite so the timeline, sessions,
//! and search can work over history rather than just the live HEAD window. Runs in the
//! background on startup and after a repo is added.

use chrono::{TimeZone, Utc};

use crate::error::{Error, Result};
use crate::git::reader;
use crate::model::{Repo, SyncReport, TimeRange};
use crate::registry::Registry;
use crate::store::Store;

/// Indexes commit history into the store.
#[derive(Clone)]
pub struct Indexer {
    store: Store,
    registry: Registry,
}

impl Indexer {
    pub fn new(store: Store, registry: Registry) -> Self {
        Self { store, registry }
    }

    /// Index all of one repo's reachable commits.
    pub async fn sync(&self, repo: &Repo) -> Result<SyncReport> {
        let path = repo.path.clone();
        let id = repo.id;
        let range = full_range();
        let commits =
            tokio::task::spawn_blocking(move || reader::read_commits_in_range(&path, id, range))
                .await
                .map_err(|e| Error::Other(format!("task failed: {e}")))??;
        let total = commits.len();
        let added = self.store.insert_commits(commits).await?;
        Ok(SyncReport {
            repo_id: id,
            commits_added: added as u32,
            commits_skipped: (total - added) as u32,
            errors: Vec::new(),
        })
    }

    /// Index every registered repo.
    pub async fn sync_all(&self) -> Result<Vec<SyncReport>> {
        let mut reports = Vec::new();
        for repo in self.registry.list().await? {
            match self.sync(&repo).await {
                Ok(r) => reports.push(r),
                Err(e) => reports.push(SyncReport {
                    repo_id: repo.id,
                    errors: vec![e.to_string()],
                    ..Default::default()
                }),
            }
        }
        Ok(reports)
    }
}

/// From the unix epoch to a year from now — i.e. "everything reachable from HEAD".
fn full_range() -> TimeRange {
    TimeRange {
        from: Utc.timestamp_opt(0, 0).single().unwrap_or_else(Utc::now),
        to: Utc::now() + chrono::Duration::days(365),
    }
}
