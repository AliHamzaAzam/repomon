//! Dyn-compatible async traits the daemon implements and clients call via JSON-RPC.
//!
//! Concrete implementations live alongside their data ([`Registry`], [`Lanes`]); these
//! traits exist so future consumers (a SwiftUI shell, a CLI helper) can depend on the
//! abstraction rather than the concrete types.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::Result;
use crate::lane::Lanes;
use crate::model::{CreateLaneParams, Lane, LaneId, Repo, RepoId};
use crate::registry::Registry;

#[async_trait]
pub trait RepoRegistry: Send + Sync {
    async fn add(&self, path: &Path) -> Result<Repo>;
    async fn remove(&self, id: RepoId) -> Result<()>;
    async fn list(&self) -> Result<Vec<Repo>>;
    async fn discover(&self, root: &Path, max_depth: usize) -> Result<Vec<PathBuf>>;
}

#[async_trait]
impl RepoRegistry for Registry {
    async fn add(&self, path: &Path) -> Result<Repo> {
        Registry::add(self, path).await
    }
    async fn remove(&self, id: RepoId) -> Result<()> {
        Registry::remove(self, id).await
    }
    async fn list(&self) -> Result<Vec<Repo>> {
        Registry::list(self).await
    }
    async fn discover(&self, root: &Path, max_depth: usize) -> Result<Vec<PathBuf>> {
        Registry::discover(self, root, max_depth).await
    }
}

#[async_trait]
pub trait LaneManager: Send + Sync {
    async fn list(&self) -> Result<Vec<Lane>>;
    async fn get(&self, id: LaneId) -> Result<Lane>;
    async fn create(&self, params: CreateLaneParams) -> Result<Lane>;
    async fn delete(&self, id: LaneId, also_delete_branch: bool) -> Result<()>;
    async fn focus(&self, id: LaneId) -> Result<PathBuf>;
}

#[async_trait]
impl LaneManager for Lanes {
    async fn list(&self) -> Result<Vec<Lane>> {
        Lanes::list(self).await
    }
    async fn get(&self, id: LaneId) -> Result<Lane> {
        Lanes::get(self, id).await
    }
    async fn create(&self, params: CreateLaneParams) -> Result<Lane> {
        Lanes::create(self, params).await
    }
    async fn delete(&self, id: LaneId, also_delete_branch: bool) -> Result<()> {
        Lanes::delete(self, id, also_delete_branch).await
    }
    async fn focus(&self, id: LaneId) -> Result<PathBuf> {
        Lanes::focus(self, id).await
    }
}
