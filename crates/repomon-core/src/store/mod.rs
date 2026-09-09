//! Serialize SQLite access on a dedicated connection-owning thread so async callers never block on
//! database work or carry a connection across await points.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::thread;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use crate::agent::supervision::SupervisionOverrides;
use crate::error::{Error, Result};
use crate::model::*;

/// Use explicit increasing schema versions rather than array indices; never renumber shipped
/// migrations or reuse reserved version numbers.
const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/0001_init.sql")),
    (2, include_str!("../../migrations/0002_agent_kind.sql")),
    (3, include_str!("../../migrations/0003_devices.sql")),
    (4, include_str!("../../migrations/0004_session_labels.sql")),
    (
        5,
        include_str!("../../migrations/0005_lanes_autoincrement.sql"),
    ),
    (6, include_str!("../../migrations/0006_remote_devices.sql")),
    (11, include_str!("../../migrations/0011_repo_hidden.sql")),
    (
        12,
        include_str!("../../migrations/0012_orchestration_log.sql"),
    ),
    (13, include_str!("../../migrations/0013_playbooks.sql")),
    (14, include_str!("../../migrations/0014_schedules.sql")),
    (15, include_str!("../../migrations/0015_approvals.sql")),
    (16, include_str!("../../migrations/0016_messages.sql")),
    (
        17,
        include_str!("../../migrations/0017_session_generated_labels.sql"),
    ),
    (18, include_str!("../../migrations/0018_supervision.sql")),
    (
        19,
        include_str!("../../migrations/0019_repo_position_label.sql"),
    ),
    (
        20,
        include_str!("../../migrations/0020_agent_session_order.sql"),
    ),
    (
        21,
        include_str!("../../migrations/0021_mcp_identity_process.sql"),
    ),
    (22, include_str!("../../migrations/0022_lane_role.sql")),
    (23, include_str!("../../migrations/0023_usage_ledger.sql")),
    (
        24,
        include_str!("../../migrations/0024_usage_headline_raw.sql"),
    ),
    (
        25,
        include_str!("../../migrations/0025_usage_headline_version.sql"),
    ),
    (
        26,
        include_str!("../../migrations/0026_usage_subagents.sql"),
    ),
    (
        27,
        include_str!("../../migrations/0027_usage_ingest_version.sql"),
    ),
    (
        28,
        include_str!("../../migrations/0028_usage_events_model.sql"),
    ),
    (
        29,
        include_str!("../../migrations/0029_usage_recount_failures.sql"),
    ),
    (
        30,
        include_str!("../../migrations/0030_message_push_attempts.sql"),
    ),
];

/// Unreviewed playbook drafts older than this are swept (opportunistically, on save/list) -
/// an unapproved draft is a proposal, not knowledge, and stale proposals shouldn't pile up.
const PLAYBOOK_DRAFT_TTL_DAYS: i64 = 30;

/// Cap on registered push devices. Re-registration refreshes a token's timestamp; beyond this the
/// oldest are evicted, so a misbehaving/abusive client can't grow the table (or per-alert APNs
/// fan-out) without bound.
const MAX_DEVICES: usize = 32;

/// Cap on paired remote devices. Unlike push tokens (evicted oldest-first), these are live access
/// credentials, so pairing a new distinct device past the cap errors rather than silently evicting
/// one - dropping a credential out from under an in-use device would lock it out without warning.
const MAX_REMOTE_DEVICES: usize = 16;

const MESSAGE_MAX_BYTES: usize = 8 * 1024;
const MESSAGE_THREAD_HOPS: u8 = 6;
const MESSAGE_PAGE_MAX: usize = 200;
const AGENT_INJECTION_DISABLED_ERROR: &str =
    "blocked: agent-to-agent injection disabled (message_inject_agents=false)";

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// A handle to the store. Cheap to clone; all clones talk to the same worker thread.
#[derive(Clone)]
pub struct Store {
    tx: Sender<Job>,
}

impl Store {
    /// Open (creating if needed) the database at `path` and run migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let path = path.to_path_buf();
        Self::spawn(move || Connection::open(&path))
    }

    /// Open an in-memory database (used by tests).
    pub fn open_in_memory() -> Result<Self> {
        Self::spawn(Connection::open_in_memory)
    }

    fn spawn<F>(open: F) -> Result<Self>
    where
        F: FnOnce() -> rusqlite::Result<Connection> + Send + 'static,
    {
        let (init_tx, init_rx) = channel::<Result<()>>();
        let (tx, rx) = channel::<Job>();
        thread::Builder::new()
            .name("repomon-store".into())
            .spawn(move || {
                let mut conn = match open() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = init_tx.send(Err(e.into()));
                        return;
                    }
                };
                if let Err(e) = init(&mut conn) {
                    let _ = init_tx.send(Err(e));
                    return;
                }
                let _ = init_tx.send(Ok(()));
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .map_err(Error::Io)?;
        init_rx
            .recv()
            .map_err(|_| Error::Other("store thread exited during init".into()))??;
        Ok(Store { tx })
    }

    /// Run a closure against the connection on the store thread and await its result.
    async fn call<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |c| {
                let _ = tx.send(f(c));
            }))
            .map_err(|_| Error::Other("store thread closed".into()))?;
        rx.await
            .map_err(|_| Error::Other("store call dropped".into()))?
    }

    pub async fn add_repo(
        &self,
        path: PathBuf,
        name: String,
        template: Option<String>,
    ) -> Result<Repo> {
        self.call(move |c| {
            let now = Utc::now();
            c.execute(
                "INSERT INTO repos(path, name, added_at, worktree_root_template) VALUES(?1, ?2, ?3, ?4)",
                params![path.to_string_lossy(), &name, to_iso(&now), &template],
            )?;
            let id = c.last_insert_rowid();
            Ok(Repo {
                id,
                path,
                name,
                added_at: now,
                worktree_root_template: template,
                hidden: false,
                position: None,
                label: None,
            })
        })
        .await
    }

    pub async fn list_repos(&self) -> Result<Vec<Repo>> {
        self.call(|c| {
            let sql = format!("SELECT {REPO_COLUMNS} FROM repos {REPO_ORDER}");
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt.query_map([], repo_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn get_repo(&self, id: RepoId) -> Result<Repo> {
        self.call(move |c| {
            let sql = format!("SELECT {REPO_COLUMNS} FROM repos WHERE id = ?1");
            c.query_row(&sql, params![id], repo_from_row)
                .map_err(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Error::NotFound(format!("repo {id}")),
                    other => other.into(),
                })
        })
        .await
    }

    pub async fn find_repo_by_path(&self, path: PathBuf) -> Result<Option<Repo>> {
        self.call(move |c| {
            let sql = format!("SELECT {REPO_COLUMNS} FROM repos WHERE path = ?1");
            let r = c
                .query_row(&sql, params![path.to_string_lossy()], repo_from_row)
                .map(Some);
            match r {
                Ok(v) => Ok(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    /// Hide or reveal a repo. Deliberately separate from `remove_repo`: hiding keeps the
    /// registration, the watches, and every lane the repo owns, so it is fully reversible.
    pub async fn set_repo_hidden(&self, id: RepoId, hidden: bool) -> Result<()> {
        self.call(move |c| {
            let n = c.execute(
                "UPDATE repos SET hidden = ?2 WHERE id = ?1",
                params![id, hidden],
            )?;
            if n == 0 {
                return Err(Error::NotFound(format!("repo {id}")));
            }
            Ok(())
        })
        .await
    }

    pub async fn remove_repo(&self, id: RepoId) -> Result<()> {
        self.call(move |c| {
            let n = c.execute("DELETE FROM repos WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(Error::NotFound(format!("repo {id}")));
            }
            Ok(())
        })
        .await
    }

    /// Persist supplied repository positions atomically while preserving omitted repositories'
    /// positions.
    pub async fn set_repo_order(&self, ordered_ids: Vec<RepoId>) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            for (index, id) in ordered_ids.iter().enumerate() {
                tx.execute(
                    "UPDATE repos SET position = ?2 WHERE id = ?1",
                    params![id, index as i64],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Set a repo's display label. `None`, empty, or whitespace-only clears the override,
    /// falling back to the folder name.
    pub async fn set_repo_label(&self, id: RepoId, label: Option<String>) -> Result<()> {
        self.call(move |c| {
            let label = label
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty());
            let n = c.execute(
                "UPDATE repos SET label = ?2 WHERE id = ?1",
                params![id, label],
            )?;
            if n == 0 {
                return Err(Error::NotFound(format!("repo {id}")));
            }
            Ok(())
        })
        .await
    }

    pub async fn upsert_worktree(
        &self,
        repo_id: RepoId,
        path: PathBuf,
        branch: Option<String>,
        head: gix::ObjectId,
        is_main: bool,
        name: String,
    ) -> Result<Worktree> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO worktrees(repo_id, path, branch, head, is_main, name)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(path) DO UPDATE SET
                     repo_id = excluded.repo_id,
                     branch  = excluded.branch,
                     head    = excluded.head,
                     is_main = excluded.is_main,
                     name    = excluded.name",
                params![
                    repo_id,
                    path.to_string_lossy(),
                    &branch,
                    oid_to_str(&head),
                    is_main as i64,
                    &name
                ],
            )?;
            let id: WorktreeId = c.query_row(
                "SELECT id FROM worktrees WHERE path = ?1",
                params![path.to_string_lossy()],
                |r| r.get(0),
            )?;
            Ok(Worktree {
                id,
                repo_id,
                path,
                branch,
                head,
                is_main,
                name,
            })
        })
        .await
    }

    pub async fn list_worktrees(&self, repo_id: RepoId) -> Result<Vec<Worktree>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT id, repo_id, path, branch, head, is_main, name
                 FROM worktrees WHERE repo_id = ?1 ORDER BY is_main DESC, name",
            )?;
            let rows = stmt.query_map(params![repo_id], worktree_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Delete worktrees of `repo_id` whose path is not in `keep`.
    pub async fn prune_worktrees(&self, repo_id: RepoId, keep: Vec<String>) -> Result<()> {
        self.call(move |c| {
            let existing: Vec<String> = {
                let mut stmt = c.prepare("SELECT path FROM worktrees WHERE repo_id = ?1")?;
                let rows = stmt.query_map(params![repo_id], |r| r.get::<_, String>(0))?;
                collect(rows)?
            };
            for p in existing {
                if !keep.contains(&p) {
                    c.execute("DELETE FROM worktrees WHERE path = ?1", params![p])?;
                }
            }
            Ok(())
        })
        .await
    }

    /// Return the stable lane id for `(repo_id, worktree_path)`, creating it if absent.
    pub async fn get_or_create_lane(
        &self,
        repo_id: RepoId,
        worktree_path: String,
    ) -> Result<LaneId> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO lanes(repo_id, worktree_path, pinned, created_at)
                 VALUES(?1, ?2, 0, ?3)
                 ON CONFLICT(repo_id, worktree_path) DO NOTHING",
                params![repo_id, worktree_path, to_iso(&Utc::now())],
            )?;
            let id: LaneId = c.query_row(
                "SELECT id FROM lanes WHERE repo_id = ?1 AND worktree_path = ?2",
                params![repo_id, worktree_path],
                |r| r.get(0),
            )?;
            Ok(id)
        })
        .await
    }

    pub async fn set_lane_pinned(&self, lane_id: LaneId, pinned: bool) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE lanes SET pinned = ?2 WHERE id = ?1",
                params![lane_id, pinned as i64],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn set_lane_tmux_window(
        &self,
        lane_id: LaneId,
        window: Option<String>,
    ) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE lanes SET tmux_window = ?2 WHERE id = ?1",
                params![lane_id, window],
            )?;
            Ok(())
        })
        .await
    }

    /// Register (or refresh) a push-notification device token.
    pub async fn register_device(&self, token: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO devices (device_token, registered_at) VALUES (?1, ?2)
                 ON CONFLICT(device_token) DO UPDATE SET registered_at = ?2",
                params![token, Utc::now().to_rfc3339()],
            )?;
            // Keep only the newest MAX_DEVICES tokens so an abusive client can't grow the table
            // (or the per-alert APNs fan-out) without bound.
            c.execute(
                "DELETE FROM devices WHERE device_token NOT IN
                 (SELECT device_token FROM devices ORDER BY registered_at DESC LIMIT ?1)",
                params![MAX_DEVICES as i64],
            )?;
            Ok(())
        })
        .await
    }

    /// Drop a device token (user unregistered, or APNs reported it dead).
    pub async fn unregister_device(&self, token: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "DELETE FROM devices WHERE device_token = ?1",
                params![token],
            )?;
            Ok(())
        })
        .await
    }

    /// All registered push device tokens.
    pub async fn list_devices(&self) -> Result<Vec<String>> {
        self.call(|c| {
            let mut stmt = c.prepare("SELECT device_token FROM devices")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            collect(rows)
        })
        .await
    }

    /// Return an existing named credential or mint a new one, refusing new names at the device cap
    /// rather than evicting live credentials.
    pub async fn remote_device_pair(&self, name: &str) -> Result<RemoteDevice> {
        let name = name.to_string();
        self.call(move |c| {
            // Existing device: return it as-is (a stable QR for the same phone).
            if let Some(dev) = remote_device_by_name(c, &name)? {
                return Ok(dev);
            }
            let count: i64 = c.query_row("SELECT COUNT(*) FROM remote_devices", [], |r| r.get(0))?;
            if count as usize >= MAX_REMOTE_DEVICES {
                return Err(Error::Other(format!(
                    "remote device limit reached ({MAX_REMOTE_DEVICES}); revoke a device before pairing a new one"
                )));
            }
            let now = Utc::now();
            let token = generate_remote_token();
            c.execute(
                "INSERT INTO remote_devices(name, token, role, created_at, last_seen_at)
                 VALUES(?1, ?2, 'full', ?3, NULL)",
                params![name, token, to_iso(&now)],
            )?;
            Ok(RemoteDevice {
                name,
                token,
                role: "full".into(),
                created_at: now,
                last_seen_at: None,
            })
        })
        .await
    }

    /// All paired remote devices, oldest first (`created_at`, ties broken by insertion order).
    pub async fn remote_device_list(&self) -> Result<Vec<RemoteDevice>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT name, token, role, created_at, last_seen_at FROM remote_devices \
                 ORDER BY created_at, id",
            )?;
            let rows = stmt.query_map([], remote_device_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Revoke a device's token by name. `Ok(false)` when no such device exists.
    pub async fn remote_device_revoke(&self, name: &str) -> Result<bool> {
        let name = name.to_string();
        self.call(move |c| {
            let n = c.execute("DELETE FROM remote_devices WHERE name = ?1", params![name])?;
            Ok(n > 0)
        })
        .await
    }

    /// Stamp a device's `last_seen_at` to now. A no-op (not an error) for an unknown name.
    pub async fn remote_device_seen(&self, name: &str) -> Result<()> {
        let name = name.to_string();
        self.call(move |c| {
            c.execute(
                "UPDATE remote_devices SET last_seen_at = ?2 WHERE name = ?1",
                params![name, to_iso(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn list_lane_meta(&self) -> Result<Vec<LaneMeta>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, repo_id, worktree_path, pinned, tmux_window, agent_kind, role \
                 FROM lanes",
            )?;
            let rows = stmt.query_map([], lane_meta_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn set_lane_agent_kind(&self, lane_id: LaneId, kind: Option<String>) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE lanes SET agent_kind = ?2 WHERE id = ?1",
                params![lane_id, kind],
            )?;
            Ok(())
        })
        .await
    }

    /// Sets or clears the lane role, including the repomind home’s controller role.
    pub async fn set_lane_role(&self, lane_id: LaneId, role: Option<String>) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE lanes SET role = ?2 WHERE id = ?1",
                params![lane_id, role],
            )?;
            Ok(())
        })
        .await
    }

    /// The lane marked `role = 'controller'`, if one exists. Ensure-home keeps exactly one, and
    /// the lowest id wins if a hand-edited database ever holds several, so the answer is stable.
    pub async fn controller_lane(&self) -> Result<Option<LaneId>> {
        self.call(|c| {
            let id = c
                .query_row(
                    "SELECT id FROM lanes WHERE role = 'controller' ORDER BY id LIMIT 1",
                    [],
                    |r| r.get::<_, LaneId>(0),
                )
                .optional()?;
            Ok(id)
        })
        .await
    }

    /// Set (or clear, when `label` is `None`) a user-defined label for a surfaced session.
    /// Transcript-backed external sessions use their session id; managed sessions use a
    /// namespaced tmux-window identity so transcript-less backends can be labelled too.
    pub async fn set_session_label(&self, session_id: String, label: Option<String>) -> Result<()> {
        self.call(move |c| {
            match label {
                Some(l) => c.execute(
                    "INSERT INTO session_labels(session_id, label, updated_at) VALUES(?1, ?2, ?3)
                     ON CONFLICT(session_id) DO UPDATE SET label = ?2, updated_at = ?3",
                    params![session_id, l, to_iso(&Utc::now())],
                )?,
                None => c.execute(
                    "DELETE FROM session_labels WHERE session_id = ?1",
                    params![session_id],
                )?,
            };
            Ok(())
        })
        .await
    }

    /// All session labels, as opaque session identity -> label.
    pub async fn list_session_labels(&self) -> Result<std::collections::HashMap<String, String>> {
        self.call(|c| {
            let mut stmt = c.prepare("SELECT session_id, label FROM session_labels")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = std::collections::HashMap::new();
            for row in rows {
                let (k, v) = row?;
                out.insert(k, v);
            }
            Ok(out)
        })
        .await
    }

    /// Persist a lane's manual agent-tab ordering: `ordered_session_ids` are assigned dense
    /// positions in one transaction (the lane's previous order is replaced wholesale, so stale
    /// entries for exited sessions never linger).
    pub async fn set_agent_session_order(
        &self,
        lane_id: LaneId,
        ordered_session_ids: Vec<String>,
    ) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM agent_session_order WHERE lane_id = ?1", params![lane_id])?;
            for (index, sid) in ordered_session_ids.iter().enumerate() {
                tx.execute(
                    "INSERT INTO agent_session_order(lane_id, session_id, position) VALUES(?1, ?2, ?3)",
                    params![lane_id, sid, index as i64],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Every lane's manual tab order, as `lane_id -> [session_id in tab order]`.
    pub async fn list_agent_session_orders(
        &self,
    ) -> Result<std::collections::HashMap<LaneId, Vec<String>>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT lane_id, session_id FROM agent_session_order ORDER BY lane_id, position",
            )?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, LaneId>(0)?, r.get::<_, String>(1)?)))?;
            let mut out: std::collections::HashMap<LaneId, Vec<String>> =
                std::collections::HashMap::new();
            for row in rows {
                let (lane_id, sid) = row?;
                out.entry(lane_id).or_default().push(sid);
            }
            Ok(out)
        })
        .await
    }

    /// Set an auto-generated local LLM label for an opaque surfaced-session identity.
    pub async fn set_session_generated_label(
        &self,
        session_id: String,
        label: String,
    ) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO session_generated_labels(session_id, label, created_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(session_id) DO UPDATE SET label = ?2, created_at = ?3",
                params![session_id, label, to_iso(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    /// Clear a generated label before a reused tmux slot is assigned to a new agent.
    pub async fn clear_session_generated_label(&self, session_id: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "DELETE FROM session_generated_labels WHERE session_id = ?1",
                params![session_id],
            )?;
            Ok(())
        })
        .await
    }

    /// All auto-generated session labels, as opaque session identity -> label.
    pub async fn list_session_generated_labels(
        &self,
    ) -> Result<std::collections::HashMap<String, String>> {
        self.call(|c| {
            let mut stmt = c.prepare("SELECT session_id, label FROM session_generated_labels")?;
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            let mut out = std::collections::HashMap::new();
            for row in rows {
                let (k, v) = row?;
                out.insert(k, v);
            }
            Ok(out)
        })
        .await
    }

    /// Mint a restricted MCP token while preserving existing tokens only for a verified matching
    /// process fingerprint, treating unverifiable identities as replacements.
    pub async fn create_mcp_identity(
        &self,
        identity: ResolvedAgentAddress,
        process_fingerprint: Option<String>,
    ) -> Result<String> {
        let token = random_hex(32);
        let token_hash = hash_identity_token(&token);
        let stored = identity.clone();
        self.call(move |c| {
            if let Some(window) = &stored.window {
                let previous_fingerprint: Option<String> = c
                    .query_row(
                        "SELECT process_fingerprint FROM mcp_identities
                         WHERE window = ?1 AND revoked_at IS NULL
                         ORDER BY created_at DESC LIMIT 1",
                        params![window],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
                let same_process = process_fingerprint.is_some()
                    && previous_fingerprint.as_deref() == process_fingerprint.as_deref();
                if !same_process {
                    c.execute(
                        "UPDATE mcp_identities SET revoked_at = ?2
                         WHERE window = ?1 AND revoked_at IS NULL",
                        params![window, to_iso(&Utc::now())],
                    )?;
                }
            }
            c.execute(
                "INSERT INTO mcp_identities(
                    token_hash, address, lane_id, slot, window, session_id, agent_kind,
                    process_fingerprint, created_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    token_hash,
                    stored.address.as_str(),
                    stored.lane_id,
                    stored.slot.map(i64::from),
                    stored.window,
                    stored.session_id,
                    stored.agent_kind,
                    process_fingerprint,
                    to_iso(&Utc::now()),
                ],
            )?;
            Ok(())
        })
        .await?;
        Ok(token)
    }

    /// Record the fingerprint of the process launched with `token` after its window exists.
    pub async fn set_mcp_identity_process_fingerprint(
        &self,
        token: String,
        process_fingerprint: String,
    ) -> Result<()> {
        let token_hash = hash_identity_token(&token);
        self.call(move |c| {
            c.execute(
                "UPDATE mcp_identities SET process_fingerprint = ?2 WHERE token_hash = ?1",
                params![token_hash, process_fingerprint],
            )?;
            Ok(())
        })
        .await
    }

    /// Resolve a plaintext MCP identity token without exposing its stored hash.
    pub async fn resolve_mcp_identity(
        &self,
        token: String,
    ) -> Result<Option<ResolvedAgentAddress>> {
        let token_hash = hash_identity_token(&token);
        self.call(move |c| {
            let result = c.query_row(
                "SELECT address, lane_id, slot, window, session_id, agent_kind
                 FROM mcp_identities WHERE token_hash = ?1 AND revoked_at IS NULL",
                params![token_hash],
                resolved_address_from_row,
            );
            match result {
                Ok(identity) => Ok(Some(identity)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
        })
        .await
    }

    pub async fn revoke_mcp_identity_for_window(&self, window: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE mcp_identities SET revoked_at = ?2
                 WHERE window = ?1 AND revoked_at IS NULL",
                params![window, to_iso(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    /// Validate, thread, rate-limit, and atomically store one message.
    pub async fn send_message(
        &self,
        requested_to: AgentAddress,
        sender: ResolvedAgentAddress,
        recipient: ResolvedAgentAddress,
        body: String,
        reply_to: Option<String>,
    ) -> Result<FleetMessage> {
        self.send_message_with_hop_budget_refresh(
            requested_to,
            sender,
            recipient,
            body,
            reply_to,
            false,
        )
        .await
    }

    /// Store one message, allowing an explicitly designated human-supervised coordinator to
    /// refresh a reply thread's hop budget. The reserved operator identity always refreshes,
    /// independent of this flag; ordinary callers should use [`Store::send_message`].
    pub async fn send_message_with_hop_budget_refresh(
        &self,
        requested_to: AgentAddress,
        sender: ResolvedAgentAddress,
        recipient: ResolvedAgentAddress,
        body: String,
        reply_to: Option<String>,
        refresh_hop_budget: bool,
    ) -> Result<FleetMessage> {
        validate_message_body(&body)?;
        let refresh_hop_budget = refresh_hop_budget || sender.address.as_str() == "operator";
        self.call(move |c| {
            let now = Utc::now();
            let one_minute_ago = to_iso(&(now - chrono::Duration::minutes(1)));
            let one_second_ago = to_iso(&(now - chrono::Duration::seconds(1)));
            let sender_address = sender.address.as_str();
            let minute_count: i64 = c.query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE sender_address = ?1 AND created_at >= ?2",
                params![sender_address, one_minute_ago],
                |r| r.get(0),
            )?;
            if minute_count >= 10 {
                return Err(Error::Other(
                    "message rate limit exceeded: ten per rolling minute".into(),
                ));
            }
            let burst_count: i64 = c.query_row(
                "SELECT COUNT(*) FROM messages
                 WHERE sender_address = ?1 AND created_at >= ?2",
                params![sender_address, one_second_ago],
                |r| r.get(0),
            )?;
            if burst_count >= 3 {
                return Err(Error::Other(
                    "message rate limit exceeded: burst of three".into(),
                ));
            }

            let explicit_parent = match reply_to.as_deref() {
                Some(id) => Some(get_message(c, id)?),
                None => None,
            };
            if let Some(parent) = &explicit_parent {
                if parent.sender.address != recipient.address
                    || parent.recipient.address != sender.address
                {
                    return Err(Error::Other(
                        "reply sender and recipient must reverse the parent message".into(),
                    ));
                }
            }
            let auto_parent = if explicit_parent.is_none() {
                recent_inbound(c, &sender.address, &recipient.address, &now)?
            } else {
                None
            };
            let parent = explicit_parent.or(auto_parent);
            let id = random_hex(16);
            let (thread_id, linked_reply, remaining_hops) = match parent {
                Some(parent) => {
                    if parent.remaining_hops == 0 && !refresh_hop_budget {
                        return Err(Error::Other("message thread hop limit exhausted".into()));
                    }
                    let remaining_hops = if refresh_hop_budget {
                        MESSAGE_THREAD_HOPS
                    } else {
                        parent.remaining_hops - 1
                    };
                    (parent.thread_id, Some(parent.id), remaining_hops)
                }
                None => (id.clone(), None, MESSAGE_THREAD_HOPS),
            };
            let message = FleetMessage {
                id,
                requested_to,
                sender,
                recipient,
                body,
                thread_id,
                reply_to: linked_reply,
                remaining_hops,
                created_at: now,
                delivered_at: None,
                read_at: None,
                delivery_error: None,
                delivery_state: MessageDeliveryState::Queued,
                read_state: MessageReadState::Unread,
            };
            insert_message(c, &message)?;
            Ok(message)
        })
        .await
    }

    /// List messages newest first. Inbox polling marks the returned rows delivered.
    pub async fn list_messages(
        &self,
        recipient: Option<AgentAddress>,
        lane_id: Option<LaneId>,
        unread_only: bool,
        limit: usize,
        before: Option<String>,
        mark_delivered: bool,
    ) -> Result<MessagePage> {
        let limit = limit.clamp(1, MESSAGE_PAGE_MAX);
        self.call(move |c| {
            let before_key = match before.as_deref() {
                Some(id) => Some(
                    c.query_row(
                        "SELECT created_at, id FROM messages WHERE id = ?1",
                        params![id],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .map_err(|e| match e {
                        rusqlite::Error::QueryReturnedNoRows => {
                            Error::NotFound(format!("message {id}"))
                        }
                        other => other.into(),
                    })?,
                ),
                None => None,
            };
            let recipient_value = recipient.as_ref().map(AgentAddress::as_str);
            let mut stmt = c.prepare(
                "SELECT id, requested_to,
                    sender_address, sender_lane_id, sender_slot, sender_window,
                    sender_session_id, sender_agent_kind,
                    recipient_address, recipient_lane_id, recipient_slot, recipient_window,
                    recipient_session_id, recipient_agent_kind,
                    body, thread_id, reply_to, remaining_hops, created_at,
                    delivered_at, read_at, delivery_error
                 FROM messages
                 WHERE (?1 IS NULL OR recipient_address = ?1)
                   AND (?2 IS NULL OR recipient_lane_id = ?2)
                   AND (?3 = 0 OR read_at IS NULL)
                   AND (?4 IS NULL OR created_at < ?4 OR (created_at = ?4 AND id < ?5))
                 ORDER BY created_at DESC, id DESC LIMIT ?6",
            )?;
            let rows = stmt.query_map(
                params![
                    recipient_value,
                    lane_id,
                    i64::from(unread_only),
                    before_key.as_ref().map(|v| v.0.as_str()),
                    before_key.as_ref().map(|v| v.1.as_str()),
                    (limit + 1) as i64,
                ],
                message_from_row,
            )?;
            let mut messages = collect(rows)?;
            let next_before = if messages.len() > limit {
                messages.truncate(limit);
                messages.last().map(|m| m.id.clone())
            } else {
                None
            };
            if mark_delivered && !messages.is_empty() {
                let delivered_at = to_iso(&Utc::now());
                for message in &mut messages {
                    if message.delivered_at.is_none() {
                        c.execute(
                            "UPDATE messages SET delivered_at = ?2, delivery_error = NULL
                             WHERE id = ?1 AND delivered_at IS NULL",
                            params![&message.id, &delivered_at],
                        )?;
                        message.delivered_at = Some(parse_iso(&delivered_at, 0)?);
                        message.delivery_error = None;
                        message.delivery_state = MessageDeliveryState::Delivered;
                    }
                }
            }
            Ok(MessagePage {
                messages,
                next_before,
            })
        })
        .await
    }

    pub async fn mark_message_read(&self, id: String) -> Result<FleetMessage> {
        self.call(move |c| {
            let now = to_iso(&Utc::now());
            let changed = c.execute(
                "UPDATE messages SET read_at = COALESCE(read_at, ?2),
                    delivered_at = COALESCE(delivered_at, ?2), delivery_error = NULL
                 WHERE id = ?1",
                params![&id, now],
            )?;
            if changed == 0 {
                return Err(Error::NotFound(format!("message {id}")));
            }
            get_message(c, &id)
        })
        .await
    }

    pub async fn get_message(&self, id: String) -> Result<FleetMessage> {
        self.call(move |c| get_message(c, &id)).await
    }

    pub async fn delete_message(&self, id: String) -> Result<()> {
        self.call(move |c| {
            let changed = c.execute("DELETE FROM messages WHERE id = ?1", params![&id])?;
            if changed == 0 {
                return Err(Error::NotFound(format!("message {id}")));
            }
            Ok(())
        })
        .await
    }

    /// Oldest queued messages whose sender class is currently allowed for pane injection.
    /// Policy-blocked mail remains durable and inbox-readable, but cannot occupy the worker's
    /// bounded delivery page and starve deliverable messages behind it.
    pub async fn queued_messages_for_injection(
        &self,
        inject_agents: bool,
        inject_operator: bool,
        limit: usize,
    ) -> Result<Vec<FleetMessage>> {
        self.call(move |c| {
            let limit = limit.clamp(1, MESSAGE_PAGE_MAX) as i64;
            if !inject_agents {
                c.execute(
                    "UPDATE messages SET delivery_error = ?1
                     WHERE id IN (
                         SELECT id FROM messages
                         WHERE delivered_at IS NULL
                           AND delivery_error IS NULL
                           AND sender_lane_id IS NOT NULL
                         ORDER BY created_at, id LIMIT ?2
                     )",
                    params![AGENT_INJECTION_DISABLED_ERROR, limit],
                )?;
            }
            let mut stmt = c.prepare(&format!(
                "SELECT {MESSAGE_COLS} FROM messages
                 WHERE delivered_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM message_push_attempts AS attempt
                       WHERE attempt.message_id = messages.id AND attempt.window = messages.recipient_window)
                   AND ((?1 = 1 AND sender_lane_id IS NOT NULL)
                     OR (?2 = 1 AND sender_lane_id IS NULL))
                 ORDER BY created_at, id LIMIT ?3"
            ))?;
            let rows = stmt.query_map(
                params![i64::from(inject_agents), i64::from(inject_operator), limit,],
                message_from_row,
            )?;
            collect(rows)
        })
        .await
    }

    /// Claim one (message, window) before terminal I/O. Keep uncertain attempts durable so a
    /// verification miss or daemon restart cannot replay a body that already reached the agent.
    pub async fn claim_message_push(&self, id: String, window: String) -> Result<bool> {
        self.call(move |c| {
            Ok(c.execute(
                "INSERT OR IGNORE INTO message_push_attempts(message_id, window, attempted_at)
                 SELECT id, ?2, ?3 FROM messages
                 WHERE id = ?1 AND delivered_at IS NULL AND recipient_window = ?2",
                params![id, window, to_iso(&Utc::now())],
            )? == 1)
        })
        .await
    }

    /// Only release a claim when the verified injector skipped without sending any input.
    pub async fn release_message_push(&self, id: String, window: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "DELETE FROM message_push_attempts WHERE message_id = ?1 AND window = ?2",
                params![id, window],
            )?;
            Ok(())
        })
        .await
    }

    /// Record a successful push injection. Since the message was typed into the recipient's
    /// live terminal, delivery and reading are the same observable event for this path.
    pub async fn mark_message_push_delivered(&self, id: String) -> Result<FleetMessage> {
        self.call(move |c| {
            let now = to_iso(&Utc::now());
            let changed = c.execute(
                "UPDATE messages SET delivered_at = COALESCE(delivered_at, ?2),
                    read_at = COALESCE(read_at, ?2), delivery_error = NULL WHERE id = ?1",
                params![&id, now],
            )?;
            if changed == 0 {
                return Err(Error::NotFound(format!("message {id}")));
            }
            get_message(c, &id)
        })
        .await
    }

    pub async fn set_message_delivery_error(&self, id: String, error: String) -> Result<()> {
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE messages SET delivery_error = ?2 WHERE id = ?1",
                params![id, error],
            )?;
            if changed == 0 {
                return Err(Error::NotFound(format!("message {id}")));
            }
            Ok(())
        })
        .await
    }

    /// Insert commits, ignoring ones already present. Returns the number newly added.
    pub async fn insert_commits(&self, commits: Vec<Commit>) -> Result<usize> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let mut added = 0usize;
            {
                let mut stmt = tx.prepare(
                    "INSERT OR IGNORE INTO commits(repo_id, oid, author_name, author_email, summary, time, parent_count)
                     VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                )?;
                for cm in &commits {
                    added += stmt.execute(params![
                        cm.repo_id,
                        oid_to_str(&cm.oid),
                        cm.author_name,
                        cm.author_email,
                        cm.summary,
                        to_iso(&cm.time),
                        cm.parent_count,
                    ])?;
                }
            }
            tx.commit()?;
            Ok(added)
        })
        .await
    }

    /// Commits within `[from, to)`, newest first, optionally filtered to `repo_ids`.
    pub async fn commits_in_range(
        &self,
        range: TimeRange,
        repo_ids: Option<Vec<RepoId>>,
    ) -> Result<Vec<Commit>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT oid, repo_id, author_name, author_email, summary, time, parent_count
                 FROM commits WHERE time >= ?1 AND time < ?2 ORDER BY time DESC",
            )?;
            let rows = stmt.query_map(
                params![to_iso(&range.from), to_iso(&range.to)],
                commit_from_row,
            )?;
            let mut out = Vec::new();
            for r in rows {
                let cm = r?;
                if let Some(ids) = &repo_ids {
                    if !ids.contains(&cm.repo_id) {
                        continue;
                    }
                }
                out.push(cm);
            }
            Ok(out)
        })
        .await
    }

    /// Search indexed commit summaries (case-insensitive substring), newest first.
    pub async fn search_commits(&self, query: String, limit: usize) -> Result<Vec<Commit>> {
        self.call(move |c| {
            let pattern = format!("%{query}%");
            let mut stmt = c.prepare(
                "SELECT oid, repo_id, author_name, author_email, summary, time, parent_count
                 FROM commits WHERE summary LIKE ?1 ORDER BY time DESC LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![pattern, limit as i64], commit_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Append one journal entry (its `id` field is ignored). Returns the assigned rowid.
    pub async fn append_journal(&self, e: JournalEntry) -> Result<i64> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO orchestration_log(at, session, action, lane_id, repo, params, outcome, detail)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    to_iso(&e.at),
                    e.session,
                    e.action,
                    e.lane_id,
                    e.repo,
                    e.params,
                    e.outcome,
                    e.detail,
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    /// The newest journal entries, newest first.
    pub async fn recent_journal(&self, limit: usize) -> Result<Vec<JournalEntry>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {JOURNAL_COLS} FROM orchestration_log ORDER BY id DESC LIMIT ?1"
            ))?;
            let rows = stmt.query_map(params![limit as i64], journal_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Search the journal (case-insensitive substring over action/repo/params/detail, the
    /// [`search_commits`](Self::search_commits) pattern), newest first.
    pub async fn search_journal(&self, query: String, limit: usize) -> Result<Vec<JournalEntry>> {
        self.call(move |c| {
            let pattern = format!("%{query}%");
            let mut stmt = c.prepare(&format!(
                "SELECT {JOURNAL_COLS} FROM orchestration_log
                 WHERE action LIKE ?1 OR repo LIKE ?1 OR params LIKE ?1 OR detail LIKE ?1
                 ORDER BY id DESC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![pattern, limit as i64], journal_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Journal rows with an id above `after`, oldest first: the repomind export's cursor read.
    /// Ascending because the export appends them to a day file in the order they happened.
    pub async fn journal_after(&self, after: i64, limit: usize) -> Result<Vec<JournalEntry>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {JOURNAL_COLS} FROM orchestration_log WHERE id > ?1
                 ORDER BY id ASC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![after, limit as i64], journal_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Return journal entries since the previous session start in ascending order, or an empty list
    /// until two session starts exist.
    pub async fn journal_since_prev_session(&self, limit: usize) -> Result<Vec<JournalEntry>> {
        self.call(move |c| {
            let anchor: Option<i64> = c
                .query_row(
                    "SELECT id FROM orchestration_log WHERE action = 'session_start'
                     ORDER BY id DESC LIMIT 1 OFFSET 1",
                    [],
                    |r| r.get(0),
                )
                .map(Some)
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })?;
            let Some(anchor) = anchor else {
                return Ok(Vec::new());
            };
            let mut stmt = c.prepare(&format!(
                "SELECT {JOURNAL_COLS} FROM orchestration_log WHERE id > ?1
                 ORDER BY id ASC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![anchor, limit as i64], journal_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Legacy SQLite playbooks for migration, in name order after expired drafts are swept.
    pub async fn list_playbooks(&self) -> Result<Vec<Playbook>> {
        self.call(|c| {
            sweep_expired_playbook_drafts(c)?;
            let mut stmt = c.prepare(&format!(
                "SELECT {PLAYBOOK_COLS} FROM playbooks ORDER BY name"
            ))?;
            let rows = stmt.query_map([], playbook_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Add a schedule. The spec is validated by the caller (`schedule::parse_spec`).
    pub async fn add_schedule(
        &self,
        spec: String,
        prompt: String,
        max_actions: u32,
    ) -> Result<Schedule> {
        self.call(move |c| {
            let now = Utc::now();
            c.execute(
                "INSERT INTO schedules(spec, prompt, max_actions, created_at)
                 VALUES(?1, ?2, ?3, ?4)",
                params![&spec, &prompt, max_actions, to_iso(&now)],
            )?;
            Ok(Schedule {
                id: c.last_insert_rowid(),
                spec,
                prompt,
                max_actions,
                created_at: now,
                last_run_at: None,
            })
        })
        .await
    }

    pub async fn list_schedules(&self) -> Result<Vec<Schedule>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, spec, prompt, max_actions, created_at, last_run_at
                 FROM schedules ORDER BY id",
            )?;
            let rows = stmt.query_map([], schedule_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn remove_schedule(&self, id: i64) -> Result<()> {
        self.call(move |c| {
            let n = c.execute("DELETE FROM schedules WHERE id = ?1", params![id])?;
            if n == 0 {
                return Err(Error::NotFound(format!("schedule {id}")));
            }
            Ok(())
        })
        .await
    }

    /// Stamp a schedule's last firing time. Called BEFORE the run so a slow run can't double-fire.
    pub async fn mark_schedule_run(&self, id: i64, at: DateTime<Utc>) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE schedules SET last_run_at = ?2 WHERE id = ?1",
                params![id, to_iso(&at)],
            )?;
            Ok(())
        })
        .await
    }

    /// Record one permission verdict and return how many CONSECUTIVE trailing approvals the
    /// (repo, pattern) now has - a deny resets the streak (denies never generalize).
    pub async fn record_approval_event(
        &self,
        repo: String,
        pattern: String,
        verdict: String,
    ) -> Result<u32> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO approval_events(repo, pattern, verdict, at) VALUES(?1, ?2, ?3, ?4)",
                params![&repo, &pattern, &verdict, to_iso(&Utc::now())],
            )?;
            let mut stmt = c.prepare(
                "SELECT verdict FROM approval_events WHERE repo = ?1 AND pattern = ?2
                 ORDER BY id DESC",
            )?;
            let rows = stmt.query_map(params![&repo, &pattern], |r| r.get::<_, String>(0))?;
            let mut streak = 0u32;
            for v in rows {
                if v?.as_str() == "approve" {
                    streak += 1;
                } else {
                    break;
                }
            }
            Ok(streak)
        })
        .await
    }

    pub async fn add_approval_rule(&self, repo: String, pattern: String) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO approval_rules(repo, pattern, created_at)
                 VALUES(?1, ?2, ?3)",
                params![&repo, &pattern, to_iso(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn remove_approval_rule(&self, repo: String, pattern: String) -> Result<()> {
        self.call(move |c| {
            let n = c.execute(
                "DELETE FROM approval_rules WHERE repo = ?1 AND pattern = ?2",
                params![&repo, &pattern],
            )?;
            if n == 0 {
                return Err(Error::NotFound(format!("approval rule {repo}:{pattern}")));
            }
            Ok(())
        })
        .await
    }

    pub async fn list_approval_rules(&self) -> Result<Vec<ApprovalRule>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT repo, pattern, created_at FROM approval_rules ORDER BY repo, pattern",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for r in rows {
                let (repo, pattern, created) = r?;
                out.push(ApprovalRule {
                    repo,
                    pattern,
                    created_at: chrono::DateTime::parse_from_rfc3339(&created)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                });
            }
            Ok(out)
        })
        .await
    }

    pub async fn has_approval_rule(&self, repo: String, pattern: String) -> Result<bool> {
        self.call(move |c| {
            let n: i64 = c.query_row(
                "SELECT COUNT(*) FROM approval_rules WHERE repo = ?1 AND pattern = ?2",
                params![&repo, &pattern],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// Insert or update a session keyed by its manifest path. Returns its id.
    pub async fn upsert_session(&self, s: AgentSession) -> Result<SessionId> {
        self.call(move |c| {
            c.execute(
                "INSERT INTO agent_sessions(agent, repo_id, worktree_id, started_at, last_activity_at, ended_at, manifest_path, tool_call_count, title)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(manifest_path) DO UPDATE SET
                     last_activity_at = excluded.last_activity_at,
                     ended_at         = excluded.ended_at,
                     tool_call_count  = excluded.tool_call_count,
                     title            = excluded.title,
                     worktree_id      = excluded.worktree_id",
                params![
                    s.agent.as_str(),
                    s.repo_id,
                    s.worktree_id,
                    to_iso(&s.started_at),
                    to_iso(&s.last_activity_at),
                    s.ended_at.map(|d| to_iso(&d)),
                    s.manifest_path.to_string_lossy(),
                    s.tool_call_count,
                    s.title,
                ],
            )?;
            let id: SessionId = c.query_row(
                "SELECT id FROM agent_sessions WHERE manifest_path = ?1",
                params![s.manifest_path.to_string_lossy()],
                |r| r.get(0),
            )?;
            Ok(id)
        })
        .await
    }

    pub async fn list_active_sessions(&self) -> Result<Vec<AgentSession>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, agent, repo_id, worktree_id, started_at, last_activity_at, ended_at, manifest_path, tool_call_count, title
                 FROM agent_sessions WHERE ended_at IS NULL ORDER BY last_activity_at DESC",
            )?;
            let rows = stmt.query_map([], session_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn end_session(&self, id: SessionId, ended_at: DateTime<Utc>) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE agent_sessions SET ended_at = ?2 WHERE id = ?1",
                params![id, to_iso(&ended_at)],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn lane_policy(&self, lane: LaneId) -> Result<Option<SupervisionOverrides>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {LANE_POLICY_COLS} FROM lane_policies WHERE lane_id = ?1"
            ))?;
            let mut rows = stmt.query_map(params![lane], lane_policy_from_row)?;
            match rows.next() {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .await
    }

    pub async fn lane_policies(&self) -> Result<Vec<SupervisionOverrides>> {
        self.call(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {LANE_POLICY_COLS} FROM lane_policies ORDER BY lane_id"
            ))?;
            let rows = stmt.query_map([], lane_policy_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn set_lane_policy(&self, p: SupervisionOverrides) -> Result<()> {
        self.call(move |c| {
            let classes_json =
                serde_json::to_string(&p.classes).unwrap_or_else(|_| "{}".to_string());
            c.execute(
                "INSERT INTO lane_policies(lane_id, enabled, classes, nudge_text, stall_mins, nudge_retries, expect_work, updated_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(lane_id) DO UPDATE SET
                     enabled       = excluded.enabled,
                     classes       = excluded.classes,
                     nudge_text    = excluded.nudge_text,
                     stall_mins    = excluded.stall_mins,
                     nudge_retries = excluded.nudge_retries,
                     expect_work   = excluded.expect_work,
                     updated_at    = excluded.updated_at",
                params![
                    p.lane_id,
                    if p.enabled { 1 } else { 0 },
                    classes_json,
                    p.nudge_text,
                    p.stall_mins.map(|v| v as i64),
                    p.nudge_retries.map(|v| v as i64),
                    if p.expect_work { 1 } else { 0 },
                    to_iso(&p.updated_at),
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn delete_lane_policy(&self, lane: LaneId) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "DELETE FROM lane_policies WHERE lane_id = ?1",
                params![lane],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn append_supervision(&self, e: SupervisionEntry) -> Result<i64> {
        self.call(move |c| {
            let dialog_class_str = e.dialog_class.map(|dc| {
                serde_json::to_string(&dc)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string()
            });
            let policy_source_str = e.policy_source.map(|ps| {
                serde_json::to_string(&ps)
                    .unwrap_or_default()
                    .trim_matches('"')
                    .to_string()
            });
            let keys_json = e.keys.and_then(|k| serde_json::to_string(&k).ok());
            let pane_excerpt = e
                .pane_excerpt
                .map(|s| truncate_char_boundary(&s, 800).to_string());

            c.execute(
                "INSERT INTO supervision_log(at, lane_id, window, session_id, agent_kind, trigger, dialog_class, repo_scoped, decision, policy_source, keys, outcome, reason, subject, pane_excerpt)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    to_iso(&e.at),
                    e.lane_id,
                    e.window,
                    e.session_id,
                    e.agent_kind,
                    e.trigger,
                    dialog_class_str,
                    e.repo_scoped.map(|b| if b { 1 } else { 0 }),
                    e.decision,
                    policy_source_str,
                    keys_json,
                    e.outcome,
                    e.reason,
                    e.subject,
                    pane_excerpt,
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    pub async fn supervision_log(
        &self,
        lane: Option<LaneId>,
        limit: usize,
        before_id: Option<i64>,
    ) -> Result<Vec<SupervisionEntry>> {
        self.call(move |c| {
            let (query, params_vec): (String, Vec<rusqlite::types::Value>) = match (lane, before_id) {
                (Some(l), Some(bid)) => (
                    format!("SELECT {SUPERVISION_COLS} FROM supervision_log WHERE lane_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3"),
                    vec![l.into(), bid.into(), (limit as i64).into()],
                ),
                (Some(l), None) => (
                    format!("SELECT {SUPERVISION_COLS} FROM supervision_log WHERE lane_id = ?1 ORDER BY id DESC LIMIT ?2"),
                    vec![l.into(), (limit as i64).into()],
                ),
                (None, Some(bid)) => (
                    format!("SELECT {SUPERVISION_COLS} FROM supervision_log WHERE id < ?1 ORDER BY id DESC LIMIT ?2"),
                    vec![bid.into(), (limit as i64).into()],
                ),
                (None, None) => (
                    format!("SELECT {SUPERVISION_COLS} FROM supervision_log ORDER BY id DESC LIMIT ?1"),
                    vec![(limit as i64).into()],
                ),
            };
            let mut stmt = c.prepare(&query)?;
            let params_slice: Vec<&dyn rusqlite::ToSql> = params_vec
                .iter()
                .map(|v| v as &dyn rusqlite::ToSql)
                .collect();
            let rows = stmt.query_map(&params_slice[..], supervision_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn supervision_last(&self, lane: LaneId) -> Result<Option<SupervisionEntry>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {SUPERVISION_COLS} FROM supervision_log WHERE lane_id = ?1 ORDER BY id DESC LIMIT 1"
            ))?;
            let mut rows = stmt.query_map(params![lane], supervision_from_row)?;
            match rows.next() {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .await
    }

    pub async fn trim_supervision_log(&self, keep: usize) -> Result<()> {
        self.call(move |c| {
            if keep == 0 {
                c.execute("DELETE FROM supervision_log", [])?;
            } else {
                c.execute(
                    "DELETE FROM supervision_log WHERE id NOT IN (
                         SELECT id FROM supervision_log ORDER BY id DESC LIMIT ?1
                     )",
                    params![keep as i64],
                )?;
            }
            Ok(())
        })
        .await
    }

    /// Atomically insert events keyed by source and offset, raise replayed token counts to their
    /// elementwise maxima, and return the number of changed rows.
    pub async fn record_usage_events(
        &self,
        events: Vec<crate::usage_ledger::UsageEvent>,
    ) -> Result<usize> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let result = record_usage_events_in(&tx, events)?;
            tx.commit()?;
            Ok(result)
        })
        .await
    }

    /// Forget every event one source contributed. Source replacement during ingest uses
    /// [`Self::commit_usage_source`] to publish the new events and cursor atomically.
    pub async fn delete_usage_events_for_source(&self, source_path: String) -> Result<usize> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let result = delete_usage_events_for_source_in(&tx, source_path)?;
            tx.commit()?;
            Ok(result)
        })
        .await
    }

    /// Publish a source read with its session metadata and cursor. A recount replaces the
    /// source's events; any failure rolls back deletion, insertion, digests, and cursor together.
    pub async fn commit_usage_source(
        &self,
        cursor: crate::usage_ledger::UsageCursor,
        replace: bool,
        events: Vec<crate::usage_ledger::UsageEvent>,
        sessions: Vec<crate::usage_ledger::UsageSessionMeta>,
    ) -> Result<usize> {
        self.call(move |c| {
            let tx = c.transaction()?;
            if replace {
                delete_usage_events_for_source_in(&tx, cursor.source_path.clone())?;
            }
            let count = record_usage_events_in(&tx, events)?;
            upsert_usage_sessions_in(&tx, sessions)?;
            set_usage_cursor_in(&tx, &cursor)?;
            tx.commit()?;
            Ok(count)
        })
        .await
    }

    /// Every ledger event in `[from, to)`, oldest first.
    pub async fn usage_events_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<crate::usage_ledger::UsageEvent>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {USAGE_EVENT_COLS} FROM usage_events
                 WHERE at >= ?1 AND at < ?2 ORDER BY at ASC, id ASC"
            ))?;
            let rows = stmt.query_map(params![to_iso(&from), to_iso(&to)], usage_event_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Where one source was last read up to.
    pub async fn usage_cursor(
        &self,
        source_path: String,
    ) -> Result<Option<crate::usage_ledger::UsageCursor>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {USAGE_CURSOR_COLS} FROM usage_ingest_cursors WHERE source_path = ?1"
            ))?;
            let mut rows = stmt.query_map(params![source_path], usage_cursor_from_row)?;
            match rows.next() {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .await
    }

    /// Every ingest cursor, newest scan first.
    pub async fn usage_cursors(&self) -> Result<Vec<crate::usage_ledger::UsageCursor>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {USAGE_CURSOR_COLS} FROM usage_ingest_cursors ORDER BY scanned_at DESC"
            ))?;
            let rows = stmt.query_map([], usage_cursor_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Stale sources independent of discovery's newest-file budget.
    pub async fn stale_usage_cursors(
        &self,
        version: u32,
        limit: usize,
    ) -> Result<Vec<crate::usage_ledger::UsageCursor>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {USAGE_CURSOR_COLS} FROM usage_ingest_cursors
                 WHERE ingest_version < ?1
                 ORDER BY ingest_version ASC, mtime DESC, source_path ASC LIMIT ?2"
            ))?;
            let rows = stmt.query_map(params![version, limit as i64], usage_cursor_from_row)?;
            collect(rows)
        })
        .await
    }

    /// Recover reader and attribution metadata for a tracked source outside current discovery roots.
    pub async fn usage_source_event(
        &self,
        path: String,
    ) -> Result<Option<crate::usage_ledger::UsageEvent>> {
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {USAGE_EVENT_COLS} FROM usage_events WHERE source_path = ?1 LIMIT 1"
            ))?;
            let mut rows = stmt.query_map(params![path], usage_event_from_row)?;
            rows.next().transpose().map_err(Into::into)
        })
        .await
    }

    /// Record where a source was read up to, which reader revision read it, and whether reading
    /// it failed.
    pub async fn set_usage_cursor(
        &self,
        source_path: String,
        offset: u64,
        mtime: i64,
        error: Option<String>,
        ingest_version: u32,
    ) -> Result<()> {
        let cursor = crate::usage_ledger::UsageCursor {
            source_path,
            offset,
            mtime,
            error,
            ingest_version,
            scanned_at: Utc::now(),
        };
        self.call(move |c| set_usage_cursor_in(c, &cursor)).await
    }

    /// Keep old events and the resume offset after a failed recount. At most one strike per
    /// minute counts toward retiring this cursor; success resets the persisted counter.
    pub async fn fail_usage_recount(
        &self,
        source_path: String,
        error: String,
        version: u32,
    ) -> Result<()> {
        self.fail_usage_recount_at(source_path, error, version, Utc::now())
            .await
    }

    async fn fail_usage_recount_at(
        &self,
        source_path: String,
        error: String,
        version: u32,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let cutoff = to_iso(&(now - chrono::Duration::seconds(60)));
        let now = to_iso(&now);
        self.call(move |c| {
            c.execute(
                "UPDATE usage_ingest_cursors SET error = ?2, scanned_at = ?3,
                    ingest_version = CASE WHEN recount_failures + 1 >= 3 THEN ?4 ELSE ingest_version END,
                    recount_failures = CASE WHEN recount_failures + 1 >= 3 THEN 0 ELSE recount_failures + 1 END
                 WHERE source_path = ?1 AND ingest_version < ?4 AND scanned_at <= ?5",
                params![source_path, error, now, version, cutoff],
            )?;
            Ok(())
        })
        .await
    }

    /// How many sources an older reader wrote and ingest has yet to re-read.
    pub async fn usage_sources_below_ingest_version(&self, version: u32) -> Result<u64> {
        self.call(move |c| {
            let mut stmt =
                c.prepare("SELECT COUNT(*) FROM usage_ingest_cursors WHERE ingest_version < ?1")?;
            let n: i64 = stmt.query_row(params![version as i64], |row| row.get(0))?;
            Ok(n.max(0) as u64)
        })
        .await
    }

    /// Merge session digests without erasing absent headlines, accumulating counters except when a
    /// newer counts version requires authoritative replacement.
    pub async fn upsert_usage_sessions(
        &self,
        rows: Vec<crate::usage_ledger::UsageSessionMeta>,
    ) -> Result<()> {
        self.call(move |c| {
            let tx = c.transaction()?;
            upsert_usage_sessions_in(&tx, rows)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// List a bounded batch of stale headline identities and optional source paths, including
    /// missing sources the caller can retire.
    pub async fn usage_sessions_needing_headline_upgrade(
        &self,
        current_version: u32,
        limit: usize,
    ) -> Result<Vec<(String, String, Option<String>)>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT agent_kind, session_id, source_path FROM usage_sessions
                 WHERE headline_version < ?1
                 ORDER BY agent_kind, session_id
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![current_version as i64, limit as i64], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            collect(rows)
        })
        .await
    }

    /// Replace a session headline authoritatively, allowing a null result to erase stale injected
    /// text.
    pub async fn update_usage_session_headline(
        &self,
        agent_kind: String,
        session_id: String,
        headline: Option<String>,
        headline_raw: Option<String>,
        version: u32,
    ) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE usage_sessions SET headline = ?1, headline_raw = ?2, headline_version = ?3
                 WHERE agent_kind = ?4 AND session_id = ?5",
                params![
                    headline,
                    headline_raw,
                    version as i64,
                    agent_kind,
                    session_id
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Retire an unreadable session from the headline-upgrade backlog without changing its content,
    /// allowing later ingest to correct it.
    pub async fn mark_usage_session_headline_current(
        &self,
        agent_kind: String,
        session_id: String,
        version: u32,
    ) -> Result<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE usage_sessions SET headline_version = ?1
                 WHERE agent_kind = ?2 AND session_id = ?3",
                params![version as i64, agent_kind, session_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Sessions with events in `[from, to)`, newest activity first. Rows come back unpriced;
    /// [`crate::usage_ledger::price_sessions`] fills in the cost.
    pub async fn usage_sessions_between(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        lane_id: Option<LaneId>,
        limit: usize,
    ) -> Result<Vec<crate::usage_ledger::UsageSessionRow>> {
        self.call(move |c| {
            // A direct predicate lets SQLite use the existing (lane_id, at) index.
            let lane_filter = if lane_id.is_some() {
                "AND e.lane_id = ?3"
            } else {
                ""
            };
            let mut stmt = c.prepare(&format!(
                "SELECT e.agent_kind, e.session_id,
                        SUM(e.input_tokens), SUM(e.output_tokens), SUM(e.cache_read_tokens),
                        SUM(e.cache_write_tokens), SUM(e.thinking_tokens),
                        SUM(CASE WHEN e.estimated = 1 THEN e.input_tokens + e.output_tokens
                                 + e.cache_read_tokens + e.cache_write_tokens ELSE 0 END),
                        SUM(CASE WHEN e.subagent = 1 THEN e.input_tokens + e.output_tokens
                                 + e.cache_read_tokens + e.cache_write_tokens ELSE 0 END),
                        COUNT(*), MIN(e.at), MAX(e.at), MAX(e.estimated), MAX(e.external),
                        MAX(e.repo_id), MAX(e.lane_id), MAX(e.cwd),
                        (SELECT x.model FROM usage_events x
                          WHERE x.agent_kind = e.agent_kind AND x.session_id = e.session_id
                          GROUP BY x.model
                          ORDER BY SUM(x.input_tokens + x.output_tokens) DESC LIMIT 1),
                        s.headline, s.turns, s.tool_calls, s.retries, s.headline_raw
                 FROM usage_events e
                 LEFT JOIN usage_sessions s
                   ON s.agent_kind = e.agent_kind AND s.session_id = e.session_id
                 WHERE e.at >= ?1 AND e.at < ?2 AND e.session_id IS NOT NULL
                   {lane_filter}
                 GROUP BY e.agent_kind, e.session_id
                 ORDER BY MAX(e.at) DESC
                 LIMIT ?4",
            ))?;
            let rows = stmt.query_map(
                params![to_iso(&from), to_iso(&to), lane_id, limit as i64],
                |row| {
                    let totals = crate::usage_ledger::UsageTotals {
                        input_tokens: row.get::<_, i64>(2)?.max(0) as u64,
                        output_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                        cache_read_tokens: row.get::<_, i64>(4)?.max(0) as u64,
                        cache_write_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                        thinking_tokens: row.get::<_, i64>(6)?.max(0) as u64,
                        estimated_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                        subagent_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                        total_tokens: (row.get::<_, i64>(2)?
                            + row.get::<_, i64>(3)?
                            + row.get::<_, i64>(4)?
                            + row.get::<_, i64>(5)?)
                        .max(0) as u64,
                        cost_usd: 0.0,
                        events: row.get::<_, i64>(9)?.max(0) as u64,
                    };
                    Ok(crate::usage_ledger::UsageSessionRow {
                        agent_kind: row.get(0)?,
                        session_id: row.get(1)?,
                        started_at: opt_dt_col(row, 10)?,
                        ended_at: opt_dt_col(row, 11)?,
                        estimated: row.get::<_, i64>(12)? != 0,
                        external: row.get::<_, i64>(13)? != 0,
                        repo_id: row.get(14)?,
                        lane_id: row.get(15)?,
                        cwd: row.get(16)?,
                        model: row.get::<_, Option<String>>(17)?.unwrap_or_default(),
                        headline: row.get(18)?,
                        turns: row.get::<_, Option<i64>>(19)?.unwrap_or(0).max(0) as u32,
                        tool_calls: row.get::<_, Option<i64>>(20)?.unwrap_or(0).max(0) as u32,
                        retries: row.get::<_, Option<i64>>(21)?.unwrap_or(0).max(0) as u32,
                        headline_raw: row.get(22)?,
                        // The store knows ids, not names: `usage_query::Labels` fills this in.
                        lane_label: None,
                        totals,
                    })
                },
            )?;
            collect(rows)
        })
        .await
    }

    /// How many events the ledger holds, and the window they span.
    pub async fn usage_extent(
        &self,
    ) -> Result<(u64, Option<DateTime<Utc>>, Option<DateTime<Utc>>)> {
        self.call(move |c| {
            let mut stmt = c.prepare("SELECT COUNT(*), MIN(at), MAX(at) FROM usage_events")?;
            let mut rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?.max(0) as u64,
                    opt_dt_col(row, 1)?,
                    opt_dt_col(row, 2)?,
                ))
            })?;
            match rows.next() {
                Some(r) => Ok(r?),
                None => Ok((0, None, None)),
            }
        })
        .await
    }

    /// Return every observed model with its latest activity and tokens since the cutoff, including
    /// models with no recent usage.
    pub async fn usage_model_seen(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<(String, Option<DateTime<Utc>>, u64)>> {
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT model, MAX(at),
                        SUM(CASE WHEN at >= ?1
                                 THEN input_tokens + output_tokens + cache_read_tokens
                                      + cache_write_tokens
                                 ELSE 0 END)
                 FROM usage_events
                 GROUP BY model
                 ORDER BY model ASC",
            )?;
            let rows = stmt.query_map(params![to_iso(&cutoff)], |row| {
                let tokens_30d: i64 = row.get(2)?;
                Ok((row.get(0)?, opt_dt_col(row, 1)?, tokens_30d.max(0) as u64))
            })?;
            collect(rows)
        })
        .await
    }
}

fn init(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )?;
    run_migrations(conn)
}

fn run_migrations(conn: &mut Connection) -> Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (target, sql) in MIGRATIONS {
        if current < *target {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", target)?;
            tx.commit()?;
        }
    }
    Ok(())
}

fn collect<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&Row) -> rusqlite::Result<T>>,
) -> Result<Vec<T>> {
    let mut v = Vec::new();
    for r in rows {
        v.push(r?);
    }
    Ok(v)
}

fn to_iso(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn oid_to_str(oid: &gix::ObjectId) -> String {
    oid.to_hex().to_string()
}

fn dt_col(row: &Row, idx: usize) -> rusqlite::Result<DateTime<Utc>> {
    let s: String = row.get(idx)?;
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                idx,
                Type::Text,
                format!("bad datetime {s:?}: {e}").into(),
            )
        })
}

fn opt_dt_col(row: &Row, idx: usize) -> rusqlite::Result<Option<DateTime<Utc>>> {
    match row.get::<_, Option<String>>(idx)? {
        None => Ok(None),
        Some(s) => DateTime::parse_from_rfc3339(&s)
            .map(|d| Some(d.with_timezone(&Utc)))
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    idx,
                    Type::Text,
                    format!("bad datetime {s:?}: {e}").into(),
                )
            }),
    }
}

fn parse_iso(value: &str, idx: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                idx,
                Type::Text,
                format!("bad datetime {value:?}: {e}").into(),
            )
        })
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    getrandom::fill(&mut value).expect("OS entropy source");
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hash_identity_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn validate_message_body(body: &str) -> Result<()> {
    if body.trim().is_empty() {
        return Err(Error::Other("message body must not be empty".into()));
    }
    if body.len() > MESSAGE_MAX_BYTES {
        return Err(Error::Other("message body exceeds 8 KiB".into()));
    }
    Ok(())
}

const MESSAGE_COLS: &str = "id, requested_to,
    sender_address, sender_lane_id, sender_slot, sender_window, sender_session_id,
    sender_agent_kind, recipient_address, recipient_lane_id, recipient_slot, recipient_window,
    recipient_session_id, recipient_agent_kind, body, thread_id, reply_to, remaining_hops,
    created_at, delivered_at, read_at, delivery_error";

fn resolved_address_from_row(row: &Row) -> rusqlite::Result<ResolvedAgentAddress> {
    Ok(ResolvedAgentAddress {
        address: AgentAddress::new(row.get::<_, String>(0)?),
        lane_id: row.get(1)?,
        slot: row.get::<_, Option<i64>>(2)?.map(|value| value as u32),
        window: row.get(3)?,
        session_id: row.get(4)?,
        agent_kind: row.get(5)?,
    })
}

fn message_from_row(row: &Row) -> rusqlite::Result<FleetMessage> {
    let delivered_at = opt_dt_col(row, 19)?;
    let read_at = opt_dt_col(row, 20)?;
    let delivery_error: Option<String> = row.get(21)?;
    let delivery_state = if delivered_at.is_some() {
        MessageDeliveryState::Delivered
    } else if delivery_error.is_some() {
        MessageDeliveryState::Failed
    } else {
        MessageDeliveryState::Queued
    };
    Ok(FleetMessage {
        id: row.get(0)?,
        requested_to: AgentAddress::new(row.get::<_, String>(1)?),
        sender: ResolvedAgentAddress {
            address: AgentAddress::new(row.get::<_, String>(2)?),
            lane_id: row.get(3)?,
            slot: row.get::<_, Option<i64>>(4)?.map(|value| value as u32),
            window: row.get(5)?,
            session_id: row.get(6)?,
            agent_kind: row.get(7)?,
        },
        recipient: ResolvedAgentAddress {
            address: AgentAddress::new(row.get::<_, String>(8)?),
            lane_id: row.get(9)?,
            slot: row.get::<_, Option<i64>>(10)?.map(|value| value as u32),
            window: row.get(11)?,
            session_id: row.get(12)?,
            agent_kind: row.get(13)?,
        },
        body: row.get(14)?,
        thread_id: row.get(15)?,
        reply_to: row.get(16)?,
        remaining_hops: row.get::<_, i64>(17)? as u8,
        created_at: dt_col(row, 18)?,
        delivered_at,
        read_at,
        delivery_error,
        delivery_state,
        read_state: if read_at.is_some() {
            MessageReadState::Read
        } else {
            MessageReadState::Unread
        },
    })
}

fn get_message(c: &Connection, id: &str) -> Result<FleetMessage> {
    c.query_row(
        &format!("SELECT {MESSAGE_COLS} FROM messages WHERE id = ?1"),
        params![id],
        message_from_row,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => Error::NotFound(format!("message {id}")),
        other => other.into(),
    })
}

fn recent_inbound(
    c: &Connection,
    sender: &AgentAddress,
    recipient: &AgentAddress,
    now: &DateTime<Utc>,
) -> Result<Option<FleetMessage>> {
    let cutoff = to_iso(&(*now - chrono::Duration::hours(24)));
    let result = c.query_row(
        &format!(
            "SELECT {MESSAGE_COLS} FROM messages
             WHERE sender_address = ?1 AND recipient_address = ?2 AND created_at >= ?3
             ORDER BY created_at DESC, id DESC LIMIT 1"
        ),
        params![recipient.as_str(), sender.as_str(), cutoff],
        message_from_row,
    );
    match result {
        Ok(message) => Ok(Some(message)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn insert_message(c: &Connection, message: &FleetMessage) -> Result<()> {
    c.execute(
        "INSERT INTO messages(
            id, requested_to, sender_address, sender_lane_id, sender_slot, sender_window,
            sender_session_id, sender_agent_kind, recipient_address, recipient_lane_id,
            recipient_slot, recipient_window, recipient_session_id, recipient_agent_kind,
            body, thread_id, reply_to, remaining_hops, created_at, delivered_at, read_at,
            delivery_error
         ) VALUES(
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
            ?17, ?18, ?19, ?20, ?21, ?22
         )",
        params![
            message.id,
            message.requested_to.as_str(),
            message.sender.address.as_str(),
            message.sender.lane_id,
            message.sender.slot.map(i64::from),
            message.sender.window,
            message.sender.session_id,
            message.sender.agent_kind,
            message.recipient.address.as_str(),
            message.recipient.lane_id,
            message.recipient.slot.map(i64::from),
            message.recipient.window,
            message.recipient.session_id,
            message.recipient.agent_kind,
            message.body,
            message.thread_id,
            message.reply_to,
            i64::from(message.remaining_hops),
            to_iso(&message.created_at),
            message.delivered_at.map(|value| to_iso(&value)),
            message.read_at.map(|value| to_iso(&value)),
            message.delivery_error,
        ],
    )?;
    Ok(())
}

fn schedule_from_row(row: &Row) -> rusqlite::Result<Schedule> {
    Ok(Schedule {
        id: row.get(0)?,
        spec: row.get(1)?,
        prompt: row.get(2)?,
        max_actions: row.get(3)?,
        created_at: dt_col(row, 4)?,
        last_run_at: opt_dt_col(row, 5)?,
    })
}

/// Column list shared by every playbook SELECT so `playbook_from_row` indexes stay in sync.
const PLAYBOOK_COLS: &str =
    "name, content, status, draft_content, created_at, updated_at, approved_at";

fn playbook_from_row(row: &Row) -> rusqlite::Result<Playbook> {
    Ok(Playbook {
        name: row.get(0)?,
        content: row.get(1)?,
        status: row.get(2)?,
        draft_content: row.get(3)?,
        created_at: dt_col(row, 4)?,
        updated_at: dt_col(row, 5)?,
        approved_at: opt_dt_col(row, 6)?,
    })
}

/// Drop unreviewed drafts older than [`PLAYBOOK_DRAFT_TTL_DAYS`]. Approved playbooks (including
/// ones carrying a pending revision) never expire.
fn sweep_expired_playbook_drafts(c: &Connection) -> Result<()> {
    let cutoff = to_iso(&(Utc::now() - chrono::Duration::days(PLAYBOOK_DRAFT_TTL_DAYS)));
    c.execute(
        "DELETE FROM playbooks WHERE status = 'draft' AND updated_at < ?1",
        params![cutoff],
    )?;
    Ok(())
}

/// Column list shared by every journal SELECT so `journal_from_row` indexes stay in sync.
/// The `usage_events` columns, in the order [`usage_event_from_row`] reads them.
const USAGE_EVENT_COLS: &str = "at, agent_kind, model, account, lane_id, repo_id, session_id, cwd, \
     window, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, thinking_tokens, \
     estimated, external, subagent, source_path, source_offset";

fn record_usage_events_in(
    tx: &rusqlite::Transaction<'_>,
    events: Vec<crate::usage_ledger::UsageEvent>,
) -> Result<usize> {
    let mut count = 0;
    {
        let mut insert = tx.prepare(&format!(
            "INSERT OR IGNORE INTO usage_events({USAGE_EVENT_COLS})
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)"
        ))?;
        let mut stored = tx.prepare(
            "SELECT input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                    thinking_tokens
             FROM usage_events WHERE source_path = ?1 AND source_offset = ?2",
        )?;
        let mut raise = tx.prepare(
            "UPDATE usage_events SET input_tokens = ?3, output_tokens = ?4,
                cache_read_tokens = ?5, cache_write_tokens = ?6, thinking_tokens = ?7
             WHERE source_path = ?1 AND source_offset = ?2",
        )?;
        for e in events {
            let n = insert.execute(params![
                to_iso(&e.at),
                e.agent_kind,
                e.model,
                e.account,
                e.lane_id,
                e.repo_id,
                e.session_id,
                e.cwd,
                e.window,
                e.input_tokens as i64,
                e.output_tokens as i64,
                e.cache_read_tokens as i64,
                e.cache_write_tokens as i64,
                e.thinking_tokens as i64,
                e.estimated as i64,
                e.external as i64,
                e.subagent as i64,
                e.source_path,
                e.source_offset,
            ])?;
            if n > 0 {
                count += 1;
                continue;
            }
            let before: (i64, i64, i64, i64, i64) =
                stored.query_row(params![e.source_path, e.source_offset], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?;
            let merged = (
                before.0.max(e.input_tokens as i64),
                before.1.max(e.output_tokens as i64),
                before.2.max(e.cache_read_tokens as i64),
                before.3.max(e.cache_write_tokens as i64),
                before.4.max(e.thinking_tokens as i64),
            );
            if merged == before {
                continue;
            }
            raise.execute(params![
                e.source_path,
                e.source_offset,
                merged.0,
                merged.1,
                merged.2,
                merged.3,
                merged.4,
            ])?;
            count += 1;
        }
    }
    Ok(count)
}

fn delete_usage_events_for_source_in(
    tx: &rusqlite::Transaction<'_>,
    source_path: String,
) -> Result<usize> {
    Ok(tx.execute(
        "DELETE FROM usage_events WHERE source_path = ?1",
        params![source_path],
    )?)
}

fn upsert_usage_sessions_in(
    tx: &rusqlite::Transaction<'_>,
    rows: Vec<crate::usage_ledger::UsageSessionMeta>,
) -> Result<()> {
    {
        let mut stmt = tx.prepare(
            "INSERT INTO usage_sessions(agent_kind, session_id, headline, headline_raw,
                headline_version, cwd, repo_id, lane_id, started_at, ended_at, turns,
                tool_calls, retries, external, source_path, counts_version)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
             ON CONFLICT(agent_kind, session_id) DO UPDATE SET
                headline = COALESCE(excluded.headline, headline),
                headline_raw = COALESCE(excluded.headline_raw, headline_raw),
                headline_version = CASE WHEN excluded.headline IS NOT NULL
                    THEN excluded.headline_version ELSE headline_version END,
                cwd = COALESCE(excluded.cwd, cwd),
                repo_id = COALESCE(excluded.repo_id, repo_id),
                lane_id = COALESCE(excluded.lane_id, lane_id),
                started_at = COALESCE(started_at, excluded.started_at),
                ended_at = COALESCE(excluded.ended_at, ended_at),
                turns = CASE WHEN counts_version < excluded.counts_version
                    THEN excluded.turns ELSE turns + excluded.turns END,
                tool_calls = CASE WHEN counts_version < excluded.counts_version
                    THEN excluded.tool_calls ELSE tool_calls + excluded.tool_calls END,
                retries = CASE WHEN counts_version < excluded.counts_version
                    THEN excluded.retries ELSE retries + excluded.retries END,
                counts_version = MAX(counts_version, excluded.counts_version),
                external = excluded.external,
                source_path = COALESCE(excluded.source_path, source_path)",
        )?;
        for r in rows {
            stmt.execute(params![
                r.agent_kind,
                r.session_id,
                r.headline,
                r.headline_raw,
                r.headline_version as i64,
                r.cwd,
                r.repo_id,
                r.lane_id,
                r.started_at.as_ref().map(to_iso),
                r.ended_at.as_ref().map(to_iso),
                r.turns as i64,
                r.tool_calls as i64,
                r.retries as i64,
                r.external as i64,
                r.source_path,
                r.counts_version as i64,
            ])?;
        }
    }
    Ok(())
}

fn set_usage_cursor_in(c: &Connection, cursor: &crate::usage_ledger::UsageCursor) -> Result<()> {
    let crate::usage_ledger::UsageCursor {
        source_path,
        offset,
        mtime,
        scanned_at,
        error,
        ingest_version,
    } = cursor;
    let now = to_iso(scanned_at);
    c.execute(
        "INSERT INTO usage_ingest_cursors(source_path, offset, mtime, scanned_at, error,
            ingest_version)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(source_path) DO UPDATE SET
            offset = excluded.offset, mtime = excluded.mtime,
            scanned_at = excluded.scanned_at, error = excluded.error,
            ingest_version = excluded.ingest_version, recount_failures = 0",
        params![
            source_path,
            *offset as i64,
            mtime,
            now,
            error,
            *ingest_version as i64
        ],
    )?;
    Ok(())
}

fn usage_event_from_row(row: &Row) -> rusqlite::Result<crate::usage_ledger::UsageEvent> {
    Ok(crate::usage_ledger::UsageEvent {
        at: dt_col(row, 0)?,
        agent_kind: row.get(1)?,
        model: row.get(2)?,
        account: row.get(3)?,
        lane_id: row.get(4)?,
        repo_id: row.get(5)?,
        session_id: row.get(6)?,
        cwd: row.get(7)?,
        window: row.get(8)?,
        input_tokens: row.get::<_, i64>(9)?.max(0) as u64,
        output_tokens: row.get::<_, i64>(10)?.max(0) as u64,
        cache_read_tokens: row.get::<_, i64>(11)?.max(0) as u64,
        cache_write_tokens: row.get::<_, i64>(12)?.max(0) as u64,
        thinking_tokens: row.get::<_, i64>(13)?.max(0) as u64,
        estimated: row.get::<_, i64>(14)? != 0,
        external: row.get::<_, i64>(15)? != 0,
        subagent: row.get::<_, i64>(16)? != 0,
        source_path: row.get(17)?,
        source_offset: row.get(18)?,
    })
}

/// The `usage_ingest_cursors` columns, in the order [`usage_cursor_from_row`] reads them.
const USAGE_CURSOR_COLS: &str = "source_path, offset, mtime, scanned_at, error, ingest_version";

fn usage_cursor_from_row(row: &Row) -> rusqlite::Result<crate::usage_ledger::UsageCursor> {
    Ok(crate::usage_ledger::UsageCursor {
        source_path: row.get(0)?,
        offset: row.get::<_, i64>(1)?.max(0) as u64,
        mtime: row.get(2)?,
        scanned_at: dt_col(row, 3)?,
        error: row.get(4)?,
        ingest_version: row.get::<_, i64>(5)?.max(0) as u32,
    })
}

const JOURNAL_COLS: &str = "id, at, session, action, lane_id, repo, params, outcome, detail";

fn journal_from_row(row: &Row) -> rusqlite::Result<JournalEntry> {
    Ok(JournalEntry {
        id: row.get(0)?,
        at: dt_col(row, 1)?,
        session: row.get(2)?,
        action: row.get(3)?,
        lane_id: row.get(4)?,
        repo: row.get(5)?,
        params: row.get(6)?,
        outcome: row.get(7)?,
        detail: row.get(8)?,
    })
}

fn oid_col(row: &Row, idx: usize) -> rusqlite::Result<gix::ObjectId> {
    let s: String = row.get(idx)?;
    s.parse::<gix::ObjectId>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            Type::Text,
            format!("bad oid {s:?}: {e}").into(),
        )
    })
}

fn remote_device_from_row(r: &Row) -> rusqlite::Result<RemoteDevice> {
    Ok(RemoteDevice {
        name: r.get(0)?,
        token: r.get(1)?,
        role: r.get(2)?,
        created_at: dt_col(r, 3)?,
        last_seen_at: opt_dt_col(r, 4)?,
    })
}

/// Look up a single remote device by its unique name, if present.
fn remote_device_by_name(c: &Connection, name: &str) -> rusqlite::Result<Option<RemoteDevice>> {
    match c.query_row(
        "SELECT name, token, role, created_at, last_seen_at FROM remote_devices WHERE name = ?1",
        params![name],
        remote_device_from_row,
    ) {
        Ok(dev) => Ok(Some(dev)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A fresh 32-byte hex bearer token from the OS entropy pool (`getrandom`, portable across
/// unix and Windows). Mirrors the CLI's `remote enable` generator
/// (`repomon-tui::cli::generate_token`).
fn generate_remote_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("OS entropy source");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn repo_from_row(r: &Row) -> rusqlite::Result<Repo> {
    Ok(Repo {
        id: r.get(0)?,
        path: PathBuf::from(r.get::<_, String>(1)?),
        name: r.get(2)?,
        added_at: dt_col(r, 3)?,
        worktree_root_template: r.get(4)?,
        hidden: r.get(5)?,
        position: r.get(6)?,
        label: r.get(7)?,
    })
}

/// The repo column list shared by every repo SELECT, matching `repo_from_row`'s indices.
const REPO_COLUMNS: &str =
    "id, path, name, added_at, worktree_root_template, hidden, position, label";

/// Orders explicitly positioned repositories first by position, then the rest by name.
const REPO_ORDER: &str = "ORDER BY (position IS NULL), position, name";

fn worktree_from_row(r: &Row) -> rusqlite::Result<Worktree> {
    Ok(Worktree {
        id: r.get(0)?,
        repo_id: r.get(1)?,
        path: PathBuf::from(r.get::<_, String>(2)?),
        branch: r.get(3)?,
        head: oid_col(r, 4)?,
        is_main: r.get::<_, i64>(5)? != 0,
        name: r.get(6)?,
    })
}

fn lane_meta_from_row(r: &Row) -> rusqlite::Result<LaneMeta> {
    Ok(LaneMeta {
        id: r.get(0)?,
        repo_id: r.get(1)?,
        worktree_path: PathBuf::from(r.get::<_, String>(2)?),
        pinned: r.get::<_, i64>(3)? != 0,
        tmux_window: r.get(4)?,
        agent_kind: r.get(5)?,
        role: r.get(6)?,
    })
}

fn commit_from_row(r: &Row) -> rusqlite::Result<Commit> {
    Ok(Commit {
        oid: oid_col(r, 0)?,
        repo_id: r.get(1)?,
        author_name: r.get(2)?,
        author_email: r.get(3)?,
        summary: r.get(4)?,
        time: dt_col(r, 5)?,
        parent_count: r.get::<_, i64>(6)? as u32,
    })
}

fn session_from_row(r: &Row) -> rusqlite::Result<AgentSession> {
    Ok(AgentSession {
        id: r.get(0)?,
        agent: AgentKind::from_kind_str(&r.get::<_, String>(1)?),
        repo_id: r.get(2)?,
        worktree_id: r.get(3)?,
        started_at: dt_col(r, 4)?,
        last_activity_at: dt_col(r, 5)?,
        ended_at: opt_dt_col(r, 6)?,
        manifest_path: PathBuf::from(r.get::<_, String>(7)?),
        tool_call_count: r.get::<_, i64>(8)? as u32,
        title: r.get(9)?,
        status: AgentStatus::Idle,
        external: false,
        session_id: None,
        resume_at: None,
        inferred: false,
        tmux_window: None,
        last_message: None,
        pending_prompt: None,
        pending_dialog: None,
        stale: false,
        stalled_since: None,
        subagent_running: None,
        status_reason: None,
        attention_kind: None,
        ended_turn: false,
        gate: None,
        config_dir: None,
        custom_label: None,
        generated_label: None,
    })
}

const LANE_POLICY_COLS: &str =
    "lane_id, enabled, classes, nudge_text, stall_mins, nudge_retries, expect_work, updated_at";

fn lane_policy_from_row(row: &Row) -> rusqlite::Result<SupervisionOverrides> {
    let lane_id: i64 = row.get(0)?;
    let enabled_int: i64 = row.get(1)?;
    let classes_str: String = row.get(2)?;
    let nudge_text: Option<String> = row.get(3)?;
    let stall_mins: Option<u32> = row.get::<_, Option<i64>>(4)?.map(|v| v as u32);
    let nudge_retries: Option<u32> = row.get::<_, Option<i64>>(5)?.map(|v| v as u32);
    let expect_work_int: i64 = row.get(6)?;
    let updated_at = dt_col(row, 7)?;

    let classes = serde_json::from_str(&classes_str).unwrap_or_default();
    Ok(SupervisionOverrides {
        lane_id,
        enabled: enabled_int != 0,
        classes,
        nudge_text,
        stall_mins,
        nudge_retries,
        expect_work: expect_work_int != 0,
        updated_at,
    })
}

const SUPERVISION_COLS: &str = "id, at, lane_id, window, session_id, agent_kind, trigger, dialog_class, repo_scoped, decision, policy_source, keys, outcome, reason, subject, pane_excerpt";

fn supervision_from_row(row: &Row) -> rusqlite::Result<SupervisionEntry> {
    let id: i64 = row.get(0)?;
    let at = dt_col(row, 1)?;
    let lane_id: i64 = row.get(2)?;
    let window: String = row.get(3)?;
    let session_id: Option<String> = row.get(4)?;
    let agent_kind: Option<String> = row.get(5)?;
    let trigger: String = row.get(6)?;
    let dialog_class_str: Option<String> = row.get(7)?;
    let repo_scoped_int: Option<i64> = row.get(8)?;
    let decision: String = row.get(9)?;
    let policy_source_str: Option<String> = row.get(10)?;
    let keys_str: Option<String> = row.get(11)?;
    let outcome: String = row.get(12)?;
    let reason: Option<String> = row.get(13)?;
    let subject: Option<String> = row.get(14)?;
    let pane_excerpt: Option<String> = row.get(15)?;

    let dialog_class =
        dialog_class_str.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok());
    let policy_source =
        policy_source_str.and_then(|s| serde_json::from_value(serde_json::Value::String(s)).ok());
    let keys = keys_str.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());

    Ok(SupervisionEntry {
        id,
        at,
        lane_id,
        window,
        session_id,
        agent_kind,
        trigger,
        dialog_class,
        repo_scoped: repo_scoped_int.map(|v| v != 0),
        decision,
        policy_source,
        keys,
        outcome,
        reason,
        subject,
        pane_excerpt,
    })
}

fn truncate_char_boundary(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        None => s,
        Some((idx, _)) => &s[..idx],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::supervision::{DialogClass, PolicyAction, PolicySource};

    fn oid(n: u8) -> gix::ObjectId {
        format!("{n:040x}").parse().unwrap()
    }

    async fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[tokio::test]
    async fn migrates_and_starts_empty() {
        let s = store().await;
        assert!(s.list_repos().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn register_device_caps_to_newest() {
        let s = store().await;
        for i in 0..(MAX_DEVICES + 5) {
            // registered_at is set to now(); distinct tokens, newest registered last.
            s.register_device(format!("token-{i:03}")).await.unwrap();
        }
        let devices = s.list_devices().await.unwrap();
        assert_eq!(devices.len(), MAX_DEVICES, "device count is capped");

        assert!(
            devices
                .iter()
                .any(|t| t == &format!("token-{:03}", MAX_DEVICES + 4))
        );
        assert!(!devices.iter().any(|t| t == "token-000"));
    }

    #[tokio::test]
    async fn remote_device_pair_mints_or_returns() {
        let s = store().await;
        let a = s.remote_device_pair("phone").await.unwrap();
        assert_eq!(a.name, "phone");
        assert_eq!(a.role, "full");
        assert!(a.token.len() >= 32, "token is 32+ bytes of entropy");
        assert!(a.last_seen_at.is_none());
        // Re-pairing the SAME name returns the identical row (a stable QR), no new insert.
        let again = s.remote_device_pair("phone").await.unwrap();
        assert_eq!(a.token, again.token);
        assert_eq!(s.remote_device_list().await.unwrap().len(), 1);

        let b = s.remote_device_pair("ipad").await.unwrap();
        assert_ne!(a.token, b.token);
        assert_eq!(s.remote_device_list().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn remote_device_pair_caps_without_evicting() {
        let s = store().await;
        for i in 0..MAX_REMOTE_DEVICES {
            s.remote_device_pair(&format!("dev-{i:02}")).await.unwrap();
        }
        // A NEW distinct name past the cap errors (credentials are never silently evicted).
        assert!(s.remote_device_pair("one-too-many").await.is_err());
        assert_eq!(
            s.remote_device_list().await.unwrap().len(),
            MAX_REMOTE_DEVICES
        );
        // Re-pairing an EXISTING name still works at the cap (returns the row, no insert).
        assert!(s.remote_device_pair("dev-00").await.is_ok());
        assert_eq!(
            s.remote_device_list().await.unwrap().len(),
            MAX_REMOTE_DEVICES
        );
    }

    #[tokio::test]
    async fn remote_device_revoke_true_then_false() {
        let s = store().await;
        s.remote_device_pair("phone").await.unwrap();
        assert!(
            s.remote_device_revoke("phone").await.unwrap(),
            "first revoke removes it"
        );
        assert!(
            !s.remote_device_revoke("phone").await.unwrap(),
            "revoking a gone device is false"
        );
        assert!(s.remote_device_list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn remote_device_seen_stamps_last_seen() {
        let s = store().await;
        s.remote_device_pair("phone").await.unwrap();
        assert!(
            s.remote_device_list().await.unwrap()[0]
                .last_seen_at
                .is_none()
        );
        s.remote_device_seen("phone").await.unwrap();
        assert!(
            s.remote_device_list().await.unwrap()[0]
                .last_seen_at
                .is_some(),
            "last_seen_at is stamped"
        );

        s.remote_device_seen("ghost").await.unwrap();
    }

    #[tokio::test]
    async fn remote_device_list_ordered_by_created() {
        let s = store().await;
        for name in ["first", "second", "third"] {
            s.remote_device_pair(name).await.unwrap();
        }
        let names: Vec<String> = s
            .remote_device_list()
            .await
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["first", "second", "third"]);
    }

    fn journal(session: &str, action: &str, params: Option<&str>) -> JournalEntry {
        JournalEntry {
            id: 0,
            at: Utc::now(),
            session: session.to_string(),
            action: action.to_string(),
            lane_id: None,
            repo: None,
            params: params.map(str::to_string),
            outcome: "ok".to_string(),
            detail: None,
        }
    }

    #[tokio::test]
    async fn journal_append_and_recent_round_trip() {
        let s = store().await;
        s.append_journal(journal("a", "session_start", None))
            .await
            .unwrap();
        s.append_journal(journal("a", "spawn_agent", Some("{\"lane_id\":1}")))
            .await
            .unwrap();
        let recent = s.recent_journal(10).await.unwrap();

        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].action, "spawn_agent");
        assert_eq!(recent[0].outcome, "ok");
        assert_eq!(recent[0].params.as_deref(), Some("{\"lane_id\":1}"));
        assert_eq!(recent[1].action, "session_start");
        assert!(recent[0].id > recent[1].id);
    }

    #[tokio::test]
    async fn journal_after_returns_only_newer_rows_oldest_first() {
        let s = store().await;
        let first = s
            .append_journal(journal("a", "session_start", None))
            .await
            .unwrap();
        s.append_journal(journal("a", "spawn_agent", None))
            .await
            .unwrap();
        s.append_journal(journal("a", "merge_lane", None))
            .await
            .unwrap();

        let rows = s.journal_after(first, 10).await.unwrap();

        let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
        assert_eq!(actions, vec!["spawn_agent", "merge_lane"]);
        assert!(rows.iter().all(|r| r.id > first));
        assert_eq!(
            s.journal_after(0, 2).await.unwrap().len(),
            2,
            "limit applies"
        );
    }

    #[tokio::test]
    async fn journal_search_is_case_insensitive_substring() {
        let s = store().await;
        s.append_journal(journal(
            "a",
            "merge_lane",
            Some("{\"lane_id\":7,\"into\":\"main\"}"),
        ))
        .await
        .unwrap();
        s.append_journal(journal(
            "a",
            "spawn_agent",
            Some("{\"task\":\"fix AUTH refactor\"}"),
        ))
        .await
        .unwrap();
        let hits = s.search_journal("auth".into(), 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, "spawn_agent");

        let hits = s.search_journal("merge".into(), 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].action, "merge_lane");
    }

    #[tokio::test]
    async fn journal_since_prev_session_spans_the_previous_session() {
        let s = store().await;
        // A session that should NOT appear (before the previous session_start).
        s.append_journal(journal("old", "session_start", None))
            .await
            .unwrap();
        s.append_journal(journal("old", "spawn_agent", None))
            .await
            .unwrap();

        s.append_journal(journal("prev", "session_start", None))
            .await
            .unwrap();
        s.append_journal(journal("prev", "create_lane", None))
            .await
            .unwrap();
        s.append_journal(journal("prev", "merge_lane", None))
            .await
            .unwrap();

        s.append_journal(journal("cur", "session_start", None))
            .await
            .unwrap();
        let recap = s.journal_since_prev_session(50).await.unwrap();
        let actions: Vec<&str> = recap.iter().map(|e| e.action.as_str()).collect();
        // Ascending, starting after the previous session_start; nothing from "old".
        assert_eq!(
            actions,
            ["create_lane", "merge_lane", "session_start"],
            "recap was: {recap:?}"
        );
        assert!(recap.iter().all(|e| e.session != "old"));
    }

    #[tokio::test]
    async fn journal_since_prev_session_empty_on_first_session() {
        let s = store().await;
        s.append_journal(journal("first", "session_start", None))
            .await
            .unwrap();
        s.append_journal(journal("first", "spawn_agent", None))
            .await
            .unwrap();
        assert!(s.journal_since_prev_session(50).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn legacy_playbook_reader_preserves_migration_fields_and_expiry_policy() {
        let s = store().await;
        s.call(|c| {
            let now = to_iso(&Utc::now());
            let old = to_iso(&(Utc::now() - chrono::Duration::days(90)));
            c.execute(
                "INSERT INTO playbooks(name, content, status, draft_content, created_at, updated_at, approved_at)
                 VALUES ('keeper', 'live', 'approved', 'revision', ?1, ?1, ?1),
                        ('stale', 'old draft', 'draft', NULL, ?1, ?1, NULL),
                        ('fresh', 'new draft', 'draft', NULL, ?2, ?2, NULL)",
                params![old, now],
            )?;
            Ok(())
        }).await.unwrap();
        let rows = s.list_playbooks().await.unwrap();
        assert_eq!(
            rows.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["fresh", "keeper"]
        );
        assert_eq!(rows[0].status, "draft");
        assert_eq!(rows[0].content, "new draft");
        assert!(rows[0].approved_at.is_none());
        assert_eq!(rows[1].status, "approved");
        assert_eq!(rows[1].content, "live");
        assert_eq!(rows[1].draft_content.as_deref(), Some("revision"));
        assert_eq!(rows[1].approved_at, Some(rows[1].created_at));
        assert_eq!(rows[1].updated_at, rows[1].created_at);
    }

    #[tokio::test]
    async fn schedule_add_list_remove_round_trip() {
        let s = store().await;
        let sched = s
            .add_schedule("daily 09:00".into(), "morning briefing".into(), 10)
            .await
            .unwrap();
        assert!(sched.id > 0);
        assert!(sched.last_run_at.is_none());
        let all = s.list_schedules().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].spec, "daily 09:00");
        assert_eq!(all[0].max_actions, 10);
        let at = Utc::now();
        s.mark_schedule_run(sched.id, at).await.unwrap();
        let all = s.list_schedules().await.unwrap();
        assert_eq!(
            all[0].last_run_at.map(|d| d.timestamp()),
            Some(at.timestamp())
        );
        s.remove_schedule(sched.id).await.unwrap();
        assert!(s.list_schedules().await.unwrap().is_empty());
        assert!(s.remove_schedule(sched.id).await.is_err());
    }

    #[tokio::test]
    async fn approval_streak_counts_consecutive_approves_and_denies_reset() {
        let s = store().await;
        for expected in [1, 2, 3] {
            let n = s
                .record_approval_event("api".into(), "cargo test".into(), "approve".into())
                .await
                .unwrap();
            assert_eq!(n, expected);
        }
        // A deny resets the streak; the next approve starts over at 1.
        assert_eq!(
            s.record_approval_event("api".into(), "cargo test".into(), "deny".into())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            s.record_approval_event("api".into(), "cargo test".into(), "approve".into())
                .await
                .unwrap(),
            1
        );

        assert_eq!(
            s.record_approval_event("web".into(), "cargo test".into(), "approve".into())
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn approval_rules_crud() {
        let s = store().await;
        assert!(
            !s.has_approval_rule("api".into(), "cargo test".into())
                .await
                .unwrap()
        );
        s.add_approval_rule("api".into(), "cargo test".into())
            .await
            .unwrap();
        s.add_approval_rule("api".into(), "cargo test".into())
            .await
            .unwrap();
        assert!(
            s.has_approval_rule("api".into(), "cargo test".into())
                .await
                .unwrap()
        );
        let rules = s.list_approval_rules().await.unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].repo, "api");
        s.remove_approval_rule("api".into(), "cargo test".into())
            .await
            .unwrap();
        assert!(
            s.remove_approval_rule("api".into(), "cargo test".into())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn reopen_file_db_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repomon.db");
        {
            let s = Store::open(&path).unwrap();
            s.add_repo(PathBuf::from("/code/x"), "x".into(), None)
                .await
                .unwrap();
            // Dropping the store ends the worker thread and closes the connection.
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // Reopening must not re-run migrations or error; data persists.
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.list_repos().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn repo_crud() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        assert_eq!(r.name, "a");
        let b = s
            .add_repo(
                PathBuf::from("/code/b"),
                "b".into(),
                Some("~/wt/{branch}".into()),
            )
            .await
            .unwrap();

        let all = s.list_repos().await.unwrap();
        assert_eq!(all.len(), 2);

        let got = s.get_repo(b.id).await.unwrap();
        assert_eq!(got.worktree_root_template.as_deref(), Some("~/wt/{branch}"));

        assert!(
            s.find_repo_by_path(PathBuf::from("/code/a"))
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            s.find_repo_by_path(PathBuf::from("/code/z"))
                .await
                .unwrap()
                .is_none()
        );

        s.remove_repo(r.id).await.unwrap();
        assert_eq!(s.list_repos().await.unwrap().len(), 1);
        assert!(matches!(s.get_repo(r.id).await, Err(Error::NotFound(_))));
    }

    #[test]
    fn migrations_reach_a_database_numbered_past_them() {
        // An existing schema version can exceed a migration's array index; explicit version targets
        // must still apply later migrations.
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE repos (
                 id                     INTEGER PRIMARY KEY,
                 path                   TEXT NOT NULL UNIQUE,
                 name                   TEXT NOT NULL,
                 added_at               TEXT NOT NULL,
                 worktree_root_template TEXT
             );
             CREATE TABLE lanes (
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 repo_id       INTEGER NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
                 worktree_path TEXT NOT NULL,
                 pinned        INTEGER NOT NULL DEFAULT 0,
                 tmux_window   TEXT,
                 created_at    TEXT NOT NULL,
                 agent_kind    TEXT,
                 UNIQUE(repo_id, worktree_path)
             );
             PRAGMA user_version = 10;",
        )
        .unwrap();

        run_migrations(&mut c).unwrap();

        let hidden: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('repos') WHERE name = 'hidden'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            hidden, 1,
            "a migration above the database's version must run"
        );
        // Read the expected version off the table rather than hardcoding it, so adding a
        // migration does not fail this test for the wrong reason.
        let newest = MIGRATIONS.iter().map(|(v, _)| *v).max().unwrap();
        let version: i64 = c
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, newest);
    }

    #[test]
    fn migration_targets_ascend_and_never_collide() {
        // Renumbering or reusing a target silently skips a migration on somebody's machine.
        let targets: Vec<i64> = MIGRATIONS.iter().map(|(v, _)| *v).collect();
        let mut sorted = targets.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            targets, sorted,
            "migration targets must be unique and ascending"
        );
    }

    #[tokio::test]
    async fn hiding_a_repo_keeps_it_listed() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        assert!(!r.hidden, "a fresh repo is visible");

        s.set_repo_hidden(r.id, true).await.unwrap();
        // Listings still carry the repo, flagged, so clients filter and can also offer a way
        // back. Removing it from the query would strand a hidden repo with no route to unhide.
        let all = s.list_repos().await.unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].hidden);
        assert!(s.get_repo(r.id).await.unwrap().hidden);

        s.set_repo_hidden(r.id, false).await.unwrap();
        assert!(!s.get_repo(r.id).await.unwrap().hidden);

        assert!(matches!(
            s.set_repo_hidden(9999, true).await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn agent_session_order_round_trip_and_replace() {
        let s = store().await;

        assert!(s.list_agent_session_orders().await.unwrap().is_empty());

        s.set_agent_session_order(7, vec!["c".into(), "a".into(), "b".into()])
            .await
            .unwrap();
        let orders = s.list_agent_session_orders().await.unwrap();
        assert_eq!(
            orders.get(&7).unwrap(),
            &vec!["c".to_string(), "a".to_string(), "b".to_string()]
        );

        s.set_agent_session_order(9, vec!["z".into()])
            .await
            .unwrap();

        // Rewriting a lane replaces its rows wholesale (no stale entries pile up).
        s.set_agent_session_order(7, vec!["b".into(), "c".into()])
            .await
            .unwrap();
        let orders = s.list_agent_session_orders().await.unwrap();
        assert_eq!(
            orders.get(&7).unwrap(),
            &vec!["b".to_string(), "c".to_string()]
        );
        assert_eq!(orders.get(&9).unwrap(), &vec!["z".to_string()]);
    }

    #[tokio::test]
    async fn generated_session_label_can_be_cleared_for_a_reused_window() {
        let s = store().await;
        let key = "win:lane-7".to_string();
        s.set_session_generated_label(key.clone(), "first-agent".into())
            .await
            .unwrap();
        assert_eq!(
            s.list_session_generated_labels().await.unwrap().get(&key),
            Some(&"first-agent".to_string())
        );

        s.clear_session_generated_label(key.clone()).await.unwrap();
        assert!(
            !s.list_session_generated_labels()
                .await
                .unwrap()
                .contains_key(&key)
        );
    }

    #[tokio::test]
    async fn repo_order_and_label_round_trip() {
        let s = store().await;
        let a = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        let b = s
            .add_repo(PathBuf::from("/code/b"), "b".into(), None)
            .await
            .unwrap();
        let c = s
            .add_repo(PathBuf::from("/code/c"), "c".into(), None)
            .await
            .unwrap();

        assert_eq!(
            s.list_repos()
                .await
                .unwrap()
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![a.id, b.id, c.id]
        );

        // A full reorder assigns dense positions and listings follow them.
        s.set_repo_order(vec![c.id, a.id, b.id]).await.unwrap();
        let listed = s.list_repos().await.unwrap();
        assert_eq!(
            listed
                .into_iter()
                .map(|r| (r.id, r.position))
                .collect::<Vec<_>>(),
            vec![(c.id, Some(0)), (a.id, Some(1)), (b.id, Some(2))]
        );

        // Repos omitted from the reorder keep their previous position.
        s.set_repo_order(vec![b.id]).await.unwrap();
        assert_eq!(s.get_repo(b.id).await.unwrap().position, Some(0));
        assert_eq!(s.get_repo(a.id).await.unwrap().position, Some(1));

        // Labels persist, and clearing (None or empty) falls back to the folder name.
        s.set_repo_label(b.id, Some("Client Portal".into()))
            .await
            .unwrap();
        assert_eq!(
            s.get_repo(b.id).await.unwrap().label.as_deref(),
            Some("Client Portal")
        );
        s.set_repo_label(b.id, Some("   ".into())).await.unwrap();
        assert!(s.get_repo(b.id).await.unwrap().label.is_none());
        s.set_repo_label(b.id, None).await.unwrap();
        assert!(s.get_repo(b.id).await.unwrap().label.is_none());

        assert!(matches!(
            s.set_repo_label(9999, Some("x".into())).await,
            Err(Error::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn lane_id_is_stable_and_pinnable() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        let l1 = s.get_or_create_lane(r.id, "/code/a".into()).await.unwrap();
        let l2 = s.get_or_create_lane(r.id, "/code/a".into()).await.unwrap();
        assert_eq!(l1, l2, "lane id must be stable for the same (repo, path)");

        let other = s
            .get_or_create_lane(r.id, "/code/a-wt/feat".into())
            .await
            .unwrap();
        assert_ne!(l1, other);

        s.set_lane_pinned(l1, true).await.unwrap();
        s.set_lane_tmux_window(l1, Some("repomon:3".into()))
            .await
            .unwrap();
        let meta = s.list_lane_meta().await.unwrap();
        let m = meta.iter().find(|m| m.id == l1).unwrap();
        assert!(m.pinned);
        assert_eq!(m.tmux_window.as_deref(), Some("repomon:3"));
    }

    #[tokio::test]
    async fn lane_ids_are_not_reused_after_delete() {
        // Regression for stale "lane-<id>" tmux windows: without AUTOINCREMENT (migration 0005)
        // SQLite reuses a freed rowid, so a re-registered worktree could inherit a still-running
        // window's id. Ids must be strictly monotonic instead.
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();

        s.get_or_create_lane(r.id, "/code/a".into()).await.unwrap();
        s.get_or_create_lane(r.id, "/code/a-wt/one".into())
            .await
            .unwrap();
        let max_id = s
            .get_or_create_lane(r.id, "/code/a-wt/two".into())
            .await
            .unwrap();

        s.call(move |c| {
            c.execute("DELETE FROM lanes WHERE id = ?1", params![max_id])?;
            Ok(())
        })
        .await
        .unwrap();

        // Re-register a worktree: the new lane must get an id ABOVE the freed one, never the
        // reused freed id.
        let new_id = s
            .get_or_create_lane(r.id, "/code/a-wt/three".into())
            .await
            .unwrap();
        assert!(
            new_id > max_id,
            "lane id {new_id} must exceed the freed max {max_id}, not be reused"
        );
    }

    #[tokio::test]
    async fn worktrees_upsert_and_prune() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        s.upsert_worktree(
            r.id,
            "/code/a".into(),
            Some("main".into()),
            oid(1),
            true,
            "main".into(),
        )
        .await
        .unwrap();
        s.upsert_worktree(
            r.id,
            "/code/a-wt/feat".into(),
            Some("feat".into()),
            oid(2),
            false,
            "feat".into(),
        )
        .await
        .unwrap();
        assert_eq!(s.list_worktrees(r.id).await.unwrap().len(), 2);

        let w = s
            .upsert_worktree(
                r.id,
                "/code/a".into(),
                Some("main".into()),
                oid(9),
                true,
                "main".into(),
            )
            .await
            .unwrap();
        assert_eq!(w.head, oid(9));
        assert_eq!(s.list_worktrees(r.id).await.unwrap().len(), 2);

        s.prune_worktrees(r.id, vec!["/code/a".into()])
            .await
            .unwrap();
        let left = s.list_worktrees(r.id).await.unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].is_main);
    }

    #[tokio::test]
    async fn commits_insert_dedupe_and_range() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        let base = Utc::now();
        let mk = |n: u8, secs: i64| Commit {
            oid: oid(n),
            repo_id: r.id,
            author_name: "ali".into(),
            author_email: "a@x".into(),
            summary: format!("commit {n}"),
            time: base - chrono::Duration::seconds(secs),
            parent_count: 1,
        };
        let added = s
            .insert_commits(vec![mk(1, 10), mk(2, 20), mk(3, 30)])
            .await
            .unwrap();
        assert_eq!(added, 3);

        let again = s.insert_commits(vec![mk(1, 10), mk(4, 40)]).await.unwrap();
        assert_eq!(again, 1);

        let range = TimeRange {
            from: base - chrono::Duration::seconds(25),
            to: base + chrono::Duration::seconds(1),
        };
        let got = s.commits_in_range(range, None).await.unwrap();
        // commits 1 and 2 fall in range; newest (smallest secs offset) first.
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].oid, oid(1));
        assert_eq!(got[1].oid, oid(2));

        let none = s
            .commits_in_range(range, Some(vec![r.id + 99]))
            .await
            .unwrap();
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn sessions_upsert_and_active() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/a"), "a".into(), None)
            .await
            .unwrap();
        let now = Utc::now();
        let sess = AgentSession {
            id: 0,
            agent: AgentKind::ClaudeCode,
            repo_id: r.id,
            worktree_id: None,
            started_at: now,
            last_activity_at: now,
            ended_at: None,
            manifest_path: PathBuf::from("/m/one.jsonl"),
            tool_call_count: 5,
            title: Some("task".into()),
            status: AgentStatus::Running,
            external: false,
            session_id: None,
            resume_at: None,
            inferred: false,
            tmux_window: None,
            last_message: None,
            pending_prompt: None,
            pending_dialog: None,
            stale: false,
            stalled_since: None,
            subagent_running: None,
            status_reason: None,
            attention_kind: None,
            ended_turn: false,
            gate: None,
            config_dir: None,
            custom_label: None,
            generated_label: None,
        };
        let id = s.upsert_session(sess.clone()).await.unwrap();
        // Upsert again (same manifest) updates rather than duplicates.
        let id2 = s
            .upsert_session(AgentSession {
                tool_call_count: 9,
                ..sess.clone()
            })
            .await
            .unwrap();
        assert_eq!(id, id2);
        let active = s.list_active_sessions().await.unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].tool_call_count, 9);

        s.end_session(id, Utc::now()).await.unwrap();
        assert!(s.list_active_sessions().await.unwrap().is_empty());
    }

    fn address(value: &str) -> ResolvedAgentAddress {
        ResolvedAgentAddress {
            address: AgentAddress::new(value),
            lane_id: value
                .strip_prefix("lane-")
                .and_then(|rest| rest.split('/').next())
                .and_then(|id| id.parse().ok()),
            slot: value
                .split_once('/')
                .and_then(|(_, slot)| slot.parse().ok()),
            window: value.starts_with("lane-").then(|| value.replace('/', "-")),
            session_id: Some(format!("session-{value}")),
            agent_kind: Some("claude-code".into()),
        }
    }

    #[tokio::test]
    async fn mcp_identity_stores_only_hash_and_replaces_only_a_different_process() {
        let s = store().await;
        let token = s
            .create_mcp_identity(address("lane-2/1"), Some("123:boot-a".into()))
            .await
            .unwrap();
        assert_eq!(
            s.resolve_mcp_identity(token.clone())
                .await
                .unwrap()
                .unwrap()
                .address
                .as_str(),
            "lane-2/1"
        );
        let token_for_query = token.clone();
        let stored: (String, i64) = s
            .call(move |c| {
                c.query_row(
                    "SELECT token_hash, COUNT(*) FROM mcp_identities WHERE token_hash = ?1",
                    params![hash_identity_token(&token_for_query)],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(Into::into)
            })
            .await
            .unwrap();
        assert_eq!(stored.1, 1);
        assert_eq!(stored.0.len(), 64);

        let second = s
            .create_mcp_identity(address("lane-2/1"), Some("123:boot-a".into()))
            .await
            .unwrap();
        assert!(s.resolve_mcp_identity(second).await.unwrap().is_some());
        assert!(
            s.resolve_mcp_identity(token.clone())
                .await
                .unwrap()
                .is_some()
        );

        let third = s
            .create_mcp_identity(address("lane-2/1"), Some("456:boot-b".into()))
            .await
            .unwrap();
        assert!(s.resolve_mcp_identity(third).await.unwrap().is_some());
        assert!(s.resolve_mcp_identity(token).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn message_store_transitions_and_inbox_delivery() {
        let s = store().await;
        let message = s
            .send_message(
                AgentAddress::new("lane-2/1"),
                address("operator"),
                address("lane-2/1"),
                "keep the full body\nwith spacing".into(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(message.delivery_state, MessageDeliveryState::Queued);
        assert_eq!(message.read_state, MessageReadState::Unread);
        assert_eq!(message.remaining_hops, MESSAGE_THREAD_HOPS);

        let page = s
            .list_messages(
                Some(AgentAddress::new("lane-2/1")),
                None,
                true,
                20,
                None,
                true,
            )
            .await
            .unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].body, "keep the full body\nwith spacing");
        assert_eq!(
            page.messages[0].delivery_state,
            MessageDeliveryState::Delivered
        );
        assert_eq!(page.messages[0].read_state, MessageReadState::Unread);
        assert!(page.messages[0].read_at.is_none());
        let read = s.mark_message_read(message.id).await.unwrap();
        assert_eq!(read.read_state, MessageReadState::Read);
        assert!(read.read_at.is_some());
    }

    #[tokio::test]
    async fn push_claim_is_atomic_and_survives_reopen_until_inbox_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.db");
        let s = Store::open(&path).unwrap();
        let body = format!("FIRST LINE\n{}\nFINAL LINE", "long body 🦀 ".repeat(400));
        let mut recipient = address("lane-2/1");
        recipient.window = Some("lane-2".into());
        let message = s
            .send_message(
                AgentAddress::new("lane-2/1"),
                address("operator"),
                recipient.clone(),
                body.clone(),
                None,
            )
            .await
            .unwrap();
        let (a, b) = tokio::join!(
            s.claim_message_push(message.id.clone(), "lane-2".into()),
            s.claim_message_push(message.id.clone(), "lane-2".into())
        );
        assert_ne!(a.unwrap(), b.unwrap());
        assert!(
            !s.claim_message_push(message.id.clone(), "lane-2-2".into())
                .await
                .unwrap()
        );
        drop(s);
        let s = Store::open(&path).unwrap();
        assert!(
            !s.claim_message_push(message.id.clone(), "lane-2".into())
                .await
                .unwrap()
        );
        assert!(
            s.queued_messages_for_injection(true, true, 50)
                .await
                .unwrap()
                .is_empty()
        );
        s.mark_message_push_delivered(message.id.clone())
            .await
            .unwrap();
        let older_id = message.id.clone();
        s.call(move |c| {
            c.execute(
                "UPDATE messages SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
                params![older_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let newer = s
            .send_message(
                AgentAddress::new("lane-2/1"),
                address("operator"),
                recipient,
                "newer".into(),
                None,
            )
            .await
            .unwrap();
        let page = s
            .list_messages(
                Some(AgentAddress::new("lane-2/1")),
                None,
                false,
                1,
                None,
                true,
            )
            .await
            .unwrap();
        assert_eq!(page.messages[0].id, newer.id);
        let page = s
            .list_messages(
                Some(AgentAddress::new("lane-2/1")),
                None,
                false,
                1,
                page.next_before,
                true,
            )
            .await
            .unwrap();
        assert_eq!(page.messages[0].body, body);
        assert_eq!(page.messages[0].id, message.id);
    }

    #[tokio::test]
    async fn push_delivery_marks_the_message_read_atomically() {
        let s = store().await;
        let message = s
            .send_message(
                AgentAddress::new("lane-2/1"),
                address("operator"),
                address("lane-2/1"),
                "inject this into the live terminal".into(),
                None,
            )
            .await
            .unwrap();

        let pushed = s.mark_message_push_delivered(message.id).await.unwrap();
        assert_eq!(pushed.delivery_state, MessageDeliveryState::Delivered);
        assert_eq!(pushed.read_state, MessageReadState::Read);
        assert_eq!(pushed.delivered_at, pushed.read_at);
    }

    #[tokio::test]
    async fn message_delete_removes_exactly_one_stored_row() {
        let s = store().await;
        let keep = s
            .send_message(
                AgentAddress::new("lane-2/1"),
                address("operator"),
                address("lane-2/1"),
                "keep".into(),
                None,
            )
            .await
            .unwrap();
        let deleted = s
            .send_message(
                AgentAddress::new("lane-3/1"),
                address("operator"),
                address("lane-3/1"),
                "delete".into(),
                None,
            )
            .await
            .unwrap();

        s.delete_message(deleted.id.clone()).await.unwrap();
        assert!(s.get_message(deleted.id.clone()).await.is_err());
        assert_eq!(s.get_message(keep.id).await.unwrap().body, "keep");
        assert!(s.delete_message(deleted.id).await.is_err());
    }

    #[tokio::test]
    async fn injection_queue_filters_sender_policy_before_the_limit() {
        let s = store().await;
        s.send_message(
            AgentAddress::new("lane-2/1"),
            address("lane-9/1"),
            address("lane-2/1"),
            "agent mail blocked by policy".into(),
            None,
        )
        .await
        .unwrap();
        s.send_message(
            AgentAddress::new("lane-2/1"),
            address("operator"),
            address("lane-2/1"),
            "operator mail remains deliverable".into(),
            None,
        )
        .await
        .unwrap();

        let operator_only = s
            .queued_messages_for_injection(false, true, 1)
            .await
            .unwrap();
        assert_eq!(operator_only.len(), 1);
        assert_eq!(operator_only[0].body, "operator mail remains deliverable");

        let agent_only = s
            .queued_messages_for_injection(true, false, 1)
            .await
            .unwrap();
        assert_eq!(agent_only.len(), 1);
        assert_eq!(agent_only[0].body, "agent mail blocked by policy");
    }

    #[tokio::test]
    async fn injection_queue_marks_agent_mail_blocked_once_and_inbox_still_reads_it() {
        let s = store().await;
        let message = s
            .send_message(
                AgentAddress::new("lane-9/1"),
                address("lane-2/1"),
                address("lane-9/1"),
                "agent mail remains available to the recipient".into(),
                None,
            )
            .await
            .unwrap();

        assert!(
            s.queued_messages_for_injection(false, true, 200)
                .await
                .unwrap()
                .is_empty()
        );
        let blocked = s.get_message(message.id.clone()).await.unwrap();
        assert_eq!(
            blocked.delivery_error.as_deref(),
            Some(AGENT_INJECTION_DISABLED_ERROR)
        );
        assert!(blocked.delivered_at.is_none());

        assert!(
            s.queued_messages_for_injection(false, true, 200)
                .await
                .unwrap()
                .is_empty()
        );
        let inbox = s
            .list_messages(
                Some(AgentAddress::new("lane-9/1")),
                None,
                false,
                20,
                None,
                true,
            )
            .await
            .unwrap();
        assert_eq!(inbox.messages.len(), 1);
        assert_eq!(
            inbox.messages[0].body,
            "agent mail remains available to the recipient"
        );
        assert_eq!(inbox.messages[0].delivery_error, None);
        assert!(inbox.messages[0].delivered_at.is_some());
    }

    #[tokio::test]
    async fn injection_queue_policy_stamp_is_bounded_and_preserves_existing_errors() {
        let s = store().await;
        let first = s
            .send_message(
                AgentAddress::new("lane-9/1"),
                address("lane-2/1"),
                address("lane-9/1"),
                "first".into(),
                None,
            )
            .await
            .unwrap();
        let second = s
            .send_message(
                AgentAddress::new("lane-9/2"),
                address("lane-2/1"),
                address("lane-9/2"),
                "second".into(),
                None,
            )
            .await
            .unwrap();
        s.set_message_delivery_error(first.id.clone(), "transient failure".into())
            .await
            .unwrap();

        s.queued_messages_for_injection(false, true, 1)
            .await
            .unwrap();
        assert_eq!(
            s.get_message(first.id)
                .await
                .unwrap()
                .delivery_error
                .as_deref(),
            Some("transient failure")
        );
        assert_eq!(
            s.get_message(second.id)
                .await
                .unwrap()
                .delivery_error
                .as_deref(),
            Some(AGENT_INJECTION_DISABLED_ERROR)
        );
    }

    #[tokio::test]
    async fn messages_validate_body_burst_and_thread_budget() {
        let s = store().await;
        assert!(
            s.send_message(
                AgentAddress::new("lane-1/1"),
                address("operator"),
                address("lane-1/1"),
                " \n ".into(),
                None,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("must not be empty")
        );
        assert!(
            s.send_message(
                AgentAddress::new("lane-1/1"),
                address("operator"),
                address("lane-1/1"),
                "x".repeat(MESSAGE_MAX_BYTES + 1),
                None,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("8 KiB")
        );

        for n in 0..3 {
            s.send_message(
                AgentAddress::new("lane-1/1"),
                address("operator"),
                address("lane-1/1"),
                format!("burst {n}"),
                None,
            )
            .await
            .unwrap();
        }
        assert!(
            s.send_message(
                AgentAddress::new("lane-1/1"),
                address("operator"),
                address("lane-1/1"),
                "burst four".into(),
                None,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("burst of three")
        );

        let s = store().await;
        let root = s
            .send_message(
                AgentAddress::new("lane-4/1"),
                address("lane-3/1"),
                address("lane-4/1"),
                "root".into(),
                None,
            )
            .await
            .unwrap();
        let mut parent = root;
        for hop in 0..MESSAGE_THREAD_HOPS {
            let old = to_iso(&(Utc::now() - chrono::Duration::seconds(2)));
            s.call(move |c| {
                c.execute("UPDATE messages SET created_at = ?1", params![old])?;
                Ok(())
            })
            .await
            .unwrap();
            let (sender, recipient) = if hop % 2 == 0 {
                (address("lane-4/1"), address("lane-3/1"))
            } else {
                (address("lane-3/1"), address("lane-4/1"))
            };
            parent = s
                .send_message(
                    recipient.address.clone(),
                    sender,
                    recipient,
                    format!("reply {hop}"),
                    Some(parent.id),
                )
                .await
                .unwrap();
        }
        assert_eq!(parent.remaining_hops, 0);
        let old = to_iso(&(Utc::now() - chrono::Duration::seconds(2)));
        s.call(move |c| {
            c.execute("UPDATE messages SET created_at = ?1", params![old])?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(
            s.send_message(
                AgentAddress::new("lane-3/1"),
                address("lane-4/1"),
                address("lane-3/1"),
                "one too far".into(),
                Some(parent.id),
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("hop limit")
        );
    }

    #[tokio::test]
    async fn operator_reply_refreshes_an_exhausted_thread_budget() {
        let s = store().await;
        let parent = s
            .send_message(
                AgentAddress::new("operator"),
                address("lane-5/1"),
                address("operator"),
                "legacy exhausted thread".into(),
                None,
            )
            .await
            .unwrap();

        // Persisted operator threads at zero remaining hops must remain replyable.
        let parent_id = parent.id.clone();
        s.call(move |c| {
            c.execute(
                "UPDATE messages SET remaining_hops = 0, created_at = ?1 WHERE id = ?2",
                params![
                    to_iso(&(Utc::now() - chrono::Duration::seconds(2))),
                    parent_id
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let operator_reply = s
            .send_message(
                AgentAddress::new("lane-5/1"),
                address("operator"),
                address("lane-5/1"),
                "continue under human supervision".into(),
                Some(parent.id.clone()),
            )
            .await
            .unwrap();
        assert_eq!(operator_reply.thread_id, parent.thread_id);
        assert_eq!(operator_reply.remaining_hops, MESSAGE_THREAD_HOPS);

        let agent_reply = s
            .send_message(
                AgentAddress::new("operator"),
                address("lane-5/1"),
                address("operator"),
                "agent response".into(),
                Some(operator_reply.id.clone()),
            )
            .await
            .unwrap();
        assert_eq!(agent_reply.thread_id, parent.thread_id);
        assert_eq!(
            agent_reply.reply_to.as_deref(),
            Some(operator_reply.id.as_str())
        );
        assert_eq!(agent_reply.remaining_hops, MESSAGE_THREAD_HOPS - 1);
    }

    #[tokio::test]
    async fn designated_coordinator_reply_refreshes_an_exhausted_thread_budget() {
        let s = store().await;
        let parent = s
            .send_message(
                AgentAddress::new("lane-81/3"),
                address("lane-1/1"),
                address("lane-81/3"),
                "approval needed".into(),
                None,
            )
            .await
            .unwrap();

        let parent_id = parent.id.clone();
        s.call(move |c| {
            c.execute(
                "UPDATE messages SET remaining_hops = 0, created_at = ?1 WHERE id = ?2",
                params![
                    to_iso(&(Utc::now() - chrono::Duration::seconds(2))),
                    parent_id
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let ordinary_error = s
            .send_message(
                AgentAddress::new("lane-1/1"),
                address("lane-81/3"),
                address("lane-1/1"),
                "ordinary agent reply remains bounded".into(),
                Some(parent.id.clone()),
            )
            .await
            .unwrap_err();
        assert!(ordinary_error.to_string().contains("hop limit"));

        let coordinator_reply = s
            .send_message_with_hop_budget_refresh(
                AgentAddress::new("lane-1/1"),
                address("lane-81/3"),
                address("lane-1/1"),
                "approved under human supervision".into(),
                Some(parent.id.clone()),
                true,
            )
            .await
            .unwrap();
        assert_eq!(coordinator_reply.thread_id, parent.thread_id);
        assert_eq!(coordinator_reply.remaining_hops, MESSAGE_THREAD_HOPS);

        let agent_reply = s
            .send_message(
                AgentAddress::new("lane-81/3"),
                address("lane-1/1"),
                address("lane-81/3"),
                "agent reply".into(),
                Some(coordinator_reply.id),
            )
            .await
            .unwrap();
        assert_eq!(agent_reply.remaining_hops, MESSAGE_THREAD_HOPS - 1);
    }

    #[tokio::test]
    async fn message_send_to_recent_inbound_peer_auto_links_thread() {
        let s = store().await;
        let inbound = s
            .send_message(
                AgentAddress::new("operator"),
                address("lane-4/1"),
                address("operator"),
                "question".into(),
                None,
            )
            .await
            .unwrap();
        let reply = s
            .send_message(
                AgentAddress::new("lane-4/1"),
                address("operator"),
                address("lane-4/1"),
                "answer without reply_to".into(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(reply.reply_to.as_deref(), Some(inbound.id.as_str()));
        assert_eq!(reply.thread_id, inbound.thread_id);
        assert_eq!(reply.remaining_hops, MESSAGE_THREAD_HOPS);
    }

    /// A lane's role is nullable and settable both ways, and `controller_lane` finds the one
    /// lane marked `controller`.
    #[tokio::test]
    async fn lane_role_round_trips_and_controller_lane_finds_it() {
        let s = store().await;
        let r = s
            .add_repo(PathBuf::from("/code/repomind"), "repomind".into(), None)
            .await
            .unwrap();
        let lane = s
            .get_or_create_lane(r.id, "/code/repomind".into())
            .await
            .unwrap();
        let other = s
            .get_or_create_lane(r.id, "/code/repomind-wt/x".into())
            .await
            .unwrap();

        assert_eq!(s.controller_lane().await.unwrap(), None);
        let meta = s.list_lane_meta().await.unwrap();
        assert!(meta.iter().all(|m| m.role.is_none()));

        s.set_lane_role(lane, Some("controller".into()))
            .await
            .unwrap();
        assert_eq!(s.controller_lane().await.unwrap(), Some(lane));
        let meta = s.list_lane_meta().await.unwrap();
        assert_eq!(
            meta.iter().find(|m| m.id == lane).unwrap().role.as_deref(),
            Some("controller")
        );
        assert!(meta.iter().find(|m| m.id == other).unwrap().role.is_none());

        s.set_lane_role(lane, None).await.unwrap();
        assert_eq!(s.controller_lane().await.unwrap(), None);
    }

    /// Migration 22 adds `lanes.role` to a database staged at 21, and a fresh database reaches
    /// the newest version with the column present.
    #[test]
    fn migration_22_applies_fresh_and_from_21() {
        let mut fresh = Connection::open_in_memory().unwrap();
        init(&mut fresh).unwrap();
        let newest = MIGRATIONS.iter().map(|(v, _)| *v).max().unwrap();
        let version: i64 = fresh
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, newest);
        assert!(newest >= 22);
        assert!(has_lane_role(&fresh));

        let mut c21 = Connection::open_in_memory().unwrap();
        for (target, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 21) {
            let tx = c21.transaction().unwrap();
            tx.execute_batch(sql).unwrap();
            tx.pragma_update(None, "user_version", target).unwrap();
            tx.commit().unwrap();
        }
        assert!(!has_lane_role(&c21));

        run_migrations(&mut c21).unwrap();
        let after: i64 = c21
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert!(
            after >= 22,
            "user_version must advance past 22, got {after}"
        );
        assert!(has_lane_role(&c21));
    }

    fn has_lane_role(c: &Connection) -> bool {
        c.prepare("SELECT role FROM lanes LIMIT 1").is_ok()
    }

    #[test]
    fn migration_18_applies_from_17() {
        let mut c17 = Connection::open_in_memory().unwrap();
        for (target, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 17) {
            let tx = c17.transaction().unwrap();
            tx.execute_batch(sql).unwrap();
            tx.pragma_update(None, "user_version", target).unwrap();
            tx.commit().unwrap();
        }
        let v17: i64 = c17
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v17, 17);

        run_migrations(&mut c17).unwrap();

        let lp_exists: i64 = c17
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='lane_policies'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lp_exists, 1);

        let sl_exists: i64 = c17
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='supervision_log'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sl_exists, 1);
    }

    #[test]
    fn migration_19_applies_fresh_and_from_18() {
        let mut c = Connection::open_in_memory().unwrap();
        init(&mut c).unwrap();
        let newest = MIGRATIONS.iter().map(|(v, _)| *v).max().unwrap();
        let version: i64 = c
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, newest);
        assert!(newest >= 19);

        let mut c18 = Connection::open_in_memory().unwrap();
        for (target, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 18) {
            let tx = c18.transaction().unwrap();
            tx.execute_batch(sql).unwrap();
            tx.pragma_update(None, "user_version", target).unwrap();
            tx.commit().unwrap();
        }
        let v18: i64 = c18
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v18, 18);

        run_migrations(&mut c18).unwrap();
        let v19: i64 = c18
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v19, newest);

        let position_exists: i64 = c18
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('repos') WHERE name='position'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(position_exists, 1);

        let label_exists: i64 = c18
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('repos') WHERE name='label'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(label_exists, 1);
    }

    #[tokio::test]
    async fn lane_policy_upsert_read_delete_roundtrip() {
        let s = store().await;
        assert_eq!(s.lane_policy(10).await.unwrap(), None);

        let mut classes = std::collections::BTreeMap::new();
        classes.insert(DialogClass::CommandExec, PolicyAction::AutoApprove);
        classes.insert(DialogClass::Deletion, PolicyAction::AutoDeny);

        let p = SupervisionOverrides {
            lane_id: 10,
            enabled: true,
            classes,
            nudge_text: Some("nudge lane".to_string()),
            stall_mins: Some(15),
            nudge_retries: Some(3),
            expect_work: true,
            updated_at: Utc::now(),
        };

        s.set_lane_policy(p.clone()).await.unwrap();

        let read = s.lane_policy(10).await.unwrap().expect("policy exists");
        assert_eq!(read.lane_id, 10);
        assert!(read.enabled);
        assert_eq!(read.classes.len(), 2);
        assert_eq!(
            read.classes.get(&DialogClass::CommandExec),
            Some(&PolicyAction::AutoApprove)
        );
        assert_eq!(
            read.classes.get(&DialogClass::Deletion),
            Some(&PolicyAction::AutoDeny)
        );
        assert_eq!(read.nudge_text.as_deref(), Some("nudge lane"));
        assert_eq!(read.stall_mins, Some(15));
        assert_eq!(read.nudge_retries, Some(3));
        assert!(read.expect_work);
        assert_eq!(read.updated_at.timestamp(), p.updated_at.timestamp());

        let all = s.lane_policies().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].lane_id, 10);

        let mut p_updated = p;
        p_updated.enabled = false;
        p_updated.classes.clear();
        s.set_lane_policy(p_updated).await.unwrap();

        let read2 = s.lane_policy(10).await.unwrap().unwrap();
        assert!(!read2.enabled);
        assert!(read2.classes.is_empty());

        s.delete_lane_policy(10).await.unwrap();
        assert_eq!(s.lane_policy(10).await.unwrap(), None);
        assert!(s.lane_policies().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn supervision_log_append_paginate_lane_filter() {
        let s = store().await;

        let make_entry = |lane_id: LaneId, trigger: &str, decision: &str| SupervisionEntry {
            id: 0,
            at: Utc::now(),
            lane_id,
            window: "repomon:1".to_string(),
            session_id: Some("sess-1".to_string()),
            agent_kind: Some("claude-code".to_string()),
            trigger: trigger.to_string(),
            dialog_class: Some(DialogClass::CommandExec),
            repo_scoped: Some(true),
            decision: decision.to_string(),
            policy_source: Some(PolicySource::ApprovalRule),
            keys: Some(vec!["Enter".to_string()]),
            outcome: "sent".to_string(),
            reason: Some("auto approved".to_string()),
            subject: Some("cargo test".to_string()),
            pane_excerpt: Some("test pane output".to_string()),
        };

        let id1 = s
            .append_supervision(make_entry(1, "dialog", "approve"))
            .await
            .unwrap();
        let id2 = s
            .append_supervision(make_entry(2, "dialog", "deny"))
            .await
            .unwrap();
        let id3 = s
            .append_supervision(make_entry(1, "stall", "nudge"))
            .await
            .unwrap();
        let id4 = s
            .append_supervision(make_entry(2, "mail", "nudge"))
            .await
            .unwrap();
        let id5 = s
            .append_supervision(make_entry(1, "dialog", "approve"))
            .await
            .unwrap();

        assert_eq!(vec![id1, id2, id3, id4, id5], vec![1, 2, 3, 4, 5]);

        let all = s.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(all.len(), 5);
        assert_eq!(
            all.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![5, 4, 3, 2, 1]
        );

        let l1 = s.supervision_log(Some(1), 10, None).await.unwrap();
        assert_eq!(l1.len(), 3);
        assert_eq!(l1.iter().map(|e| e.id).collect::<Vec<_>>(), vec![5, 3, 1]);

        let l2 = s.supervision_log(Some(2), 10, None).await.unwrap();
        assert_eq!(l2.len(), 2);
        assert_eq!(l2.iter().map(|e| e.id).collect::<Vec<_>>(), vec![4, 2]);

        let page1 = s.supervision_log(None, 2, None).await.unwrap();
        assert_eq!(page1.iter().map(|e| e.id).collect::<Vec<_>>(), vec![5, 4]);

        let page2 = s
            .supervision_log(None, 2, Some(page1.last().unwrap().id))
            .await
            .unwrap();
        assert_eq!(page2.iter().map(|e| e.id).collect::<Vec<_>>(), vec![3, 2]);

        let page3 = s
            .supervision_log(None, 2, Some(page2.last().unwrap().id))
            .await
            .unwrap();
        assert_eq!(page3.iter().map(|e| e.id).collect::<Vec<_>>(), vec![1]);

        let last1 = s.supervision_last(1).await.unwrap().expect("last exists");
        assert_eq!(last1.id, 5);
        let last2 = s.supervision_last(2).await.unwrap().expect("last exists");
        assert_eq!(last2.id, 4);
        let last3 = s.supervision_last(999).await.unwrap();
        assert!(last3.is_none());
    }

    #[tokio::test]
    async fn supervision_log_trim_keeps_newest() {
        let s = store().await;

        let make_entry = |idx: usize| SupervisionEntry {
            id: 0,
            at: Utc::now(),
            lane_id: 1,
            window: "w".to_string(),
            session_id: None,
            agent_kind: None,
            trigger: "dialog".to_string(),
            dialog_class: None,
            repo_scoped: None,
            decision: "approve".to_string(),
            policy_source: None,
            keys: None,
            outcome: "sent".to_string(),
            reason: Some(format!("entry {idx}")),
            subject: None,
            pane_excerpt: None,
        };

        for i in 1..=10 {
            s.append_supervision(make_entry(i)).await.unwrap();
        }
        assert_eq!(s.supervision_log(None, 20, None).await.unwrap().len(), 10);

        s.trim_supervision_log(3).await.unwrap();
        let remaining = s.supervision_log(None, 20, None).await.unwrap();
        assert_eq!(remaining.len(), 3);
        assert_eq!(
            remaining.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![10, 9, 8]
        );
    }

    #[tokio::test]
    async fn pane_excerpt_truncated_to_800() {
        let s = store().await;

        let long_excerpt = "a".repeat(1200);
        let e = SupervisionEntry {
            id: 0,
            at: Utc::now(),
            lane_id: 1,
            window: "w".to_string(),
            session_id: None,
            agent_kind: None,
            trigger: "dialog".to_string(),
            dialog_class: None,
            repo_scoped: None,
            decision: "approve".to_string(),
            policy_source: None,
            keys: None,
            outcome: "sent".to_string(),
            reason: None,
            subject: None,
            pane_excerpt: Some(long_excerpt),
        };

        let rowid = s.append_supervision(e).await.unwrap();
        let fetched = s.supervision_last(1).await.unwrap().unwrap();
        assert_eq!(fetched.id, rowid);
        let stored_excerpt = fetched.pane_excerpt.unwrap();
        assert_eq!(stored_excerpt.len(), 800);
        assert_eq!(stored_excerpt, "a".repeat(800));

        let unicode_long = "🦀".repeat(1000); // each emoji is 4 bytes
        let e2 = SupervisionEntry {
            id: 0,
            at: Utc::now(),
            lane_id: 2,
            window: "w".to_string(),
            session_id: None,
            agent_kind: None,
            trigger: "dialog".to_string(),
            dialog_class: None,
            repo_scoped: None,
            decision: "approve".to_string(),
            policy_source: None,
            keys: None,
            outcome: "sent".to_string(),
            reason: None,
            subject: None,
            pane_excerpt: Some(unicode_long),
        };
        s.append_supervision(e2).await.unwrap();
        let fetched2 = s.supervision_last(2).await.unwrap().unwrap();
        let stored_unicode = fetched2.pane_excerpt.unwrap();
        assert_eq!(stored_unicode.chars().count(), 800);
        assert_eq!(stored_unicode, "🦀".repeat(800));
    }

    fn usage_event(offset: i64, model: &str, at: &str) -> crate::usage_ledger::UsageEvent {
        crate::usage_ledger::UsageEvent {
            at: chrono::DateTime::parse_from_rfc3339(at)
                .unwrap()
                .with_timezone(&chrono::Utc),
            agent_kind: "claude-code".to_string(),
            model: model.to_string(),
            account: "default".to_string(),
            lane_id: Some(3),
            repo_id: Some(1),
            session_id: Some("sess-1".to_string()),
            window: None,
            cwd: Some("/repos/demo".to_string()),
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
            thinking_tokens: 5,
            estimated: false,
            external: false,
            subagent: false,
            source_path: "/t/s.jsonl".to_string(),
            source_offset: offset,
        }
    }

    #[tokio::test]
    async fn usage_events_round_trip_through_the_ledger() {
        let s = store().await;
        let n = s
            .record_usage_events(vec![usage_event(
                0,
                "claude-sonnet-5",
                "2026-09-01T10:00:00Z",
            )])
            .await
            .unwrap();
        assert_eq!(n, 1);
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let to = from + chrono::Duration::days(1);
        let rows = s.usage_events_between(from, to).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model, "claude-sonnet-5");
        assert_eq!(rows[0].cache_write_tokens, 40);
        assert_eq!(rows[0].lane_id, Some(3));
        assert_eq!(rows[0].cwd.as_deref(), Some("/repos/demo"));
    }

    #[tokio::test]
    async fn usage_model_seen_reports_last_seen_and_thirty_day_tokens_per_model() {
        let s = store().await;
        s.record_usage_events(vec![
            usage_event(0, "claude-sonnet-5", "2026-09-01T10:00:00Z"),
            usage_event(1, "claude-sonnet-5", "2026-01-01T00:00:00Z"),
            usage_event(2, "gpt-5", "2026-08-20T00:00:00Z"),
        ])
        .await
        .unwrap();
        let cutoff = chrono::DateTime::parse_from_rfc3339("2026-08-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = s.usage_model_seen(cutoff).await.unwrap();
        let sonnet = rows.iter().find(|(m, ..)| m == "claude-sonnet-5").unwrap();
        assert_eq!(
            sonnet.1,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-09-01T10:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            )
        );
        // One event is inside the 30-day-ish cutoff (Sep 1), one is well before it (Jan 1): only
        // the in-window event's tokens count. Each event contributes 10+20+30+40 = 100 tokens.
        assert_eq!(sonnet.2, 100);
        let gpt = rows.iter().find(|(m, ..)| m == "gpt-5").unwrap();
        assert_eq!(gpt.2, 100, "gpt-5's one event is inside the cutoff too");
    }

    #[tokio::test]
    async fn recording_the_same_source_offset_twice_inserts_once() {
        let s = store().await;
        let e = usage_event(0, "claude-sonnet-5", "2026-09-01T10:00:00Z");
        assert_eq!(s.record_usage_events(vec![e.clone()]).await.unwrap(), 1);
        assert_eq!(s.record_usage_events(vec![e]).await.unwrap(), 0);
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert_eq!(
            s.usage_events_between(from, from + chrono::Duration::days(1))
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_message_re_read_keeps_the_elementwise_maximum_counts() {
        // A Claude message that gained a block since the last pass comes back at the same offset
        // with a larger count; the ledger must hold the larger one, once.
        let s = store().await;
        let mut e = usage_event(0, "claude-sonnet-5", "2026-09-01T10:00:00Z");
        e.output_tokens = 7;
        assert_eq!(s.record_usage_events(vec![e.clone()]).await.unwrap(), 1);
        e.output_tokens = 250;
        assert_eq!(s.record_usage_events(vec![e.clone()]).await.unwrap(), 1);
        e.input_tokens = 1;
        e.output_tokens = 20;
        assert_eq!(s.record_usage_events(vec![e]).await.unwrap(), 0);
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = s
            .usage_events_between(from, from + chrono::Duration::days(1))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "one message, one row");
        assert_eq!(rows[0].output_tokens, 250);
        assert_eq!(rows[0].input_tokens, 10);
    }

    #[tokio::test]
    async fn dropping_a_sources_events_returns_the_deleted_count() {
        let s = store().await;
        s.record_usage_events(vec![usage_event(
            0,
            "claude-sonnet-5",
            "2026-09-01T10:00:00Z",
        )])
        .await
        .unwrap();
        assert_eq!(
            s.delete_usage_events_for_source("/t/s.jsonl".into())
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            s.delete_usage_events_for_source("/t/s.jsonl".into())
                .await
                .unwrap(),
            0
        );
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(
            s.usage_events_between(from, from + chrono::Duration::days(1))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_sessions_row_reports_the_share_its_subagents_spent() {
        let s = store().await;
        let mut sub = usage_event(1, "claude-sonnet-5", "2026-09-01T10:01:00Z");
        sub.subagent = true;
        sub.source_path = "/t/s/subagents/agent-1.jsonl".to_string();
        s.record_usage_events(vec![
            usage_event(0, "claude-sonnet-5", "2026-09-01T10:00:00Z"),
            sub,
        ])
        .await
        .unwrap();
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = s
            .usage_sessions_between(from, from + chrono::Duration::days(1), None, 50)
            .await
            .unwrap();
        let row = rows.first().expect("the session both turns belong to");
        assert_eq!(row.totals.total_tokens, 200, "both turns fold into one row");
        assert_eq!(
            row.totals.subagent_tokens, 100,
            "half of it was spent by a subagent"
        );
    }

    #[tokio::test]
    async fn an_ingest_cursor_survives_a_round_trip_and_records_an_error() {
        let s = store().await;
        assert!(s.usage_cursor("/t/s.jsonl".into()).await.unwrap().is_none());
        s.set_usage_cursor("/t/s.jsonl".into(), 512, 99, None, 1)
            .await
            .unwrap();
        let c = s.usage_cursor("/t/s.jsonl".into()).await.unwrap().unwrap();
        assert_eq!(c.offset, 512);
        assert_eq!(c.mtime, 99);
        assert!(c.error.is_none());
        s.set_usage_cursor("/t/s.jsonl".into(), 512, 99, Some("unreadable".into()), 1)
            .await
            .unwrap();
        let c = s.usage_cursor("/t/s.jsonl".into()).await.unwrap().unwrap();
        assert_eq!(c.error.as_deref(), Some("unreadable"));
        assert_eq!(s.usage_cursors().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recount_failures_survive_reopen_and_success_resets_the_streak() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("usage.db");
        let path = "/t/recount.jsonl".to_string();
        let s = Store::open(&db).unwrap();
        s.set_usage_cursor(path.clone(), 512, 99, None, 0)
            .await
            .unwrap();
        let start = s
            .usage_cursor(path.clone())
            .await
            .unwrap()
            .unwrap()
            .scanned_at;
        for seconds in [0, 59, 60, 60, 119, 120] {
            s.fail_usage_recount_at(
                path.clone(),
                "unreadable".into(),
                1,
                start + chrono::Duration::seconds(seconds),
            )
            .await
            .unwrap();
        }
        let cursor = s.usage_cursor(path.clone()).await.unwrap().unwrap();
        assert_eq!(cursor.ingest_version, 0);
        assert_eq!(cursor.scanned_at, start + chrono::Duration::seconds(120));
        drop(s);
        let s = Store::open(&db).unwrap();
        s.fail_usage_recount_at(
            path.clone(),
            "too soon".into(),
            1,
            start + chrono::Duration::seconds(179),
        )
        .await
        .unwrap();
        assert_eq!(
            s.usage_cursor(path.clone())
                .await
                .unwrap()
                .unwrap()
                .ingest_version,
            0
        );
        s.fail_usage_recount_at(
            path.clone(),
            "third strike".into(),
            1,
            start + chrono::Duration::seconds(180),
        )
        .await
        .unwrap();
        let cursor = s.usage_cursor(path.clone()).await.unwrap().unwrap();
        assert_eq!(cursor.ingest_version, 1);
        assert_eq!(cursor.offset, 512);
        assert_eq!(cursor.error.as_deref(), Some("third strike"));

        // A successful read clears the two strikes accumulated for the next reader version.
        for seconds in [240, 300] {
            s.fail_usage_recount_at(
                path.clone(),
                "retry".into(),
                2,
                start + chrono::Duration::seconds(seconds),
            )
            .await
            .unwrap();
        }
        s.set_usage_cursor(path.clone(), 1024, 100, None, 2)
            .await
            .unwrap();
        let reset = s
            .usage_cursor(path.clone())
            .await
            .unwrap()
            .unwrap()
            .scanned_at;
        for seconds in [60, 120] {
            s.fail_usage_recount_at(
                path.clone(),
                "new failure".into(),
                3,
                reset + chrono::Duration::seconds(seconds),
            )
            .await
            .unwrap();
            assert_eq!(
                s.usage_cursor(path.clone())
                    .await
                    .unwrap()
                    .unwrap()
                    .ingest_version,
                2
            );
        }
        s.fail_usage_recount_at(
            path.clone(),
            "third failure".into(),
            3,
            reset + chrono::Duration::seconds(180),
        )
        .await
        .unwrap();
        assert_eq!(
            s.usage_cursor(path).await.unwrap().unwrap().ingest_version,
            3
        );
    }

    #[tokio::test]
    async fn source_replacement_rolls_back_events_digests_and_cursor_then_recovers_after_reopen() {
        for failure in ["insert", "cursor"] {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("usage.db");
            let store = Store::open(&db).unwrap();
            let old = usage_event(0, "old", "2026-09-01T10:00:00Z");
            let source = old.source_path.clone();
            let from = old.at - chrono::Duration::hours(1);
            let to = old.at + chrono::Duration::hours(1);
            let session = crate::usage_ledger::UsageSessionMeta {
                session_id: old.session_id.clone().unwrap(),
                agent_kind: old.agent_kind.clone(),
                headline: Some("old headline".into()),
                headline_raw: None,
                headline_version: 0,
                cwd: None,
                repo_id: None,
                lane_id: None,
                started_at: None,
                ended_at: None,
                turns: 7,
                tool_calls: 3,
                retries: 1,
                external: false,
                source_path: Some(source.clone()),
                counts_version: 0,
            };
            store.record_usage_events(vec![old.clone()]).await.unwrap();
            store
                .upsert_usage_sessions(vec![session.clone()])
                .await
                .unwrap();
            store
                .set_usage_cursor(source.clone(), 512, 1, None, 0)
                .await
                .unwrap();
            let before_cursor = store.usage_cursor(source.clone()).await.unwrap().unwrap();
            store.call(move |c| {
                c.execute_batch(if failure == "insert" {
                    "CREATE TRIGGER fail_publish BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT, 'injected insert failure'); END;"
                } else {
                    "CREATE TRIGGER fail_publish BEFORE UPDATE ON usage_ingest_cursors BEGIN SELECT RAISE(ABORT, 'injected cursor failure'); END;"
                })?;
                Ok(())
            }).await.unwrap();
            let mut replacement = old.clone();
            replacement.model = "replacement".into();
            replacement.input_tokens = 1;
            let new_session = crate::usage_ledger::UsageSessionMeta {
                turns: 2,
                counts_version: 1,
                headline: Some("new headline".into()),
                ..session
            };
            let cursor = crate::usage_ledger::UsageCursor {
                offset: 1024,
                ingest_version: 1,
                ..before_cursor.clone()
            };
            assert!(
                store
                    .commit_usage_source(
                        cursor.clone(),
                        true,
                        vec![replacement.clone()],
                        vec![new_session.clone()]
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                store.usage_events_between(from, to).await.unwrap(),
                vec![old.clone()]
            );
            assert_eq!(
                store.usage_cursor(source.clone()).await.unwrap().unwrap(),
                before_cursor
            );
            let rows = store
                .usage_sessions_between(from, to, None, 10)
                .await
                .unwrap();
            assert_eq!(rows[0].turns, 7);
            assert_eq!(rows[0].headline.as_deref(), Some("old headline"));
            drop(store);
            let store = Store::open(&db).unwrap();
            assert_eq!(
                store.usage_events_between(from, to).await.unwrap(),
                vec![old]
            );
            store
                .call(|c| {
                    c.execute_batch("DROP TRIGGER fail_publish")?;
                    Ok(())
                })
                .await
                .unwrap();
            store
                .commit_usage_source(
                    cursor.clone(),
                    true,
                    vec![replacement.clone()],
                    vec![new_session],
                )
                .await
                .unwrap();
            assert_eq!(
                store.usage_events_between(from, to).await.unwrap(),
                vec![replacement]
            );
            assert_eq!(store.usage_cursor(source).await.unwrap().unwrap(), cursor);
            let rows = store
                .usage_sessions_between(from, to, None, 10)
                .await
                .unwrap();
            assert_eq!(rows[0].turns, 2);
            assert_eq!(rows[0].headline.as_deref(), Some("new headline"));
        }
    }

    #[tokio::test]
    async fn session_rows_carry_the_headline_and_totals_of_their_events() {
        let s = store().await;
        s.record_usage_events(vec![usage_event(
            0,
            "claude-sonnet-5",
            "2026-09-01T10:00:00Z",
        )])
        .await
        .unwrap();
        s.upsert_usage_sessions(vec![crate::usage_ledger::UsageSessionMeta {
            session_id: "sess-1".to_string(),
            agent_kind: "claude-code".to_string(),
            headline: Some("Wire up the ledger".to_string()),
            headline_raw: Some("Wire up the ledger, please".to_string()),
            headline_version: crate::usage_ledger::HEADLINE_VERSION,
            cwd: Some("/repos/demo".to_string()),
            repo_id: Some(1),
            lane_id: Some(3),
            started_at: None,
            ended_at: None,
            turns: 4,
            tool_calls: 2,
            retries: 1,
            external: false,
            source_path: Some("/t/s.jsonl".to_string()),
            counts_version: crate::usage_ledger::INGEST_VERSION,
        }])
        .await
        .unwrap();
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = s
            .usage_sessions_between(from, from + chrono::Duration::days(1), None, 50)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].headline.as_deref(), Some("Wire up the ledger"));
        assert_eq!(rows[0].retries, 1);
        assert_eq!(rows[0].model, "claude-sonnet-5");
        assert_eq!(rows[0].totals.input_tokens, 10);
        assert_eq!(rows[0].totals.events, 1);
    }

    #[tokio::test]
    async fn session_rows_can_be_filtered_to_one_lane() {
        let s = store().await;
        let mut other = usage_event(0, "claude-sonnet-5", "2026-09-01T10:00:00Z");
        other.lane_id = Some(9);
        other.session_id = Some("sess-9".to_string());
        s.record_usage_events(vec![
            usage_event(200, "claude-sonnet-5", "2026-09-01T10:00:00Z"),
            other,
        ])
        .await
        .unwrap();
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = s
            .usage_sessions_between(from, from + chrono::Duration::days(1), Some(9), 50)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "sess-9");
    }

    #[tokio::test]
    async fn session_lane_filter_preserves_aggregates_metadata_and_range_limits() {
        let s = store().await;
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let to = from + chrono::Duration::days(1);
        let mut events = Vec::new();
        for (offset, hours, lane, session, kind) in [
            (0, 0, Some(3), Some("shared"), "claude-code"),
            (1, 1, Some(3), Some("shared"), "claude-code"),
            (2, 2, Some(9), Some("shared"), "claude-code"),
            (3, 3, None, Some("external"), "claude-code"),
            (4, 4, Some(3), None, "claude-code"),
            (5, 24, Some(3), Some("shared"), "claude-code"),
            (6, -1, Some(3), Some("older"), "claude-code"),
            (7, 5, Some(3), Some("shared"), "codex"),
        ] {
            let mut e = usage_event(offset, "model-a", "2026-09-01T00:00:00Z");
            e.at = from + chrono::Duration::hours(hours);
            e.lane_id = lane;
            e.session_id = session.map(str::to_string);
            e.agent_kind = kind.into();
            e.estimated = offset == 0;
            e.subagent = offset == 0;
            e.external = lane.is_none();
            if offset == 2 {
                e.model = "model-b".into();
                e.input_tokens = 1_000;
            }
            events.push(e);
        }
        s.record_usage_events(events).await.unwrap();
        s.call(|c| {
            c.execute("INSERT INTO usage_sessions(agent_kind, session_id, headline, headline_raw, turns, tool_calls, retries)
                VALUES ('claude-code', 'shared', 'Task', 'Raw task', 4, 2, 1)", [])?;
            Ok(())
        }).await.unwrap();

        let filtered = s
            .usage_sessions_between(from, to, Some(3), 50)
            .await
            .unwrap();
        assert_eq!(
            filtered.len(),
            2,
            "agent kind separates matching session ids"
        );
        assert_eq!(filtered[0].agent_kind, "codex", "newest activity first");
        assert!(filtered[0].headline.is_none(), "a digest is optional");
        let row = &filtered[1];
        assert_eq!(row.session_id, "shared");
        assert_eq!(row.lane_id, Some(3));
        assert_eq!(row.started_at, Some(from), "the lower bound is inclusive");
        assert_eq!(row.ended_at, Some(from + chrono::Duration::hours(1)));
        assert_eq!(
            row.totals.events, 2,
            "other lanes, null sessions, and the upper bound are excluded"
        );
        assert_eq!(row.totals.input_tokens, 20);
        assert_eq!(row.totals.output_tokens, 40);
        assert_eq!(row.totals.cache_read_tokens, 60);
        assert_eq!(row.totals.cache_write_tokens, 80);
        assert_eq!(row.totals.thinking_tokens, 10);
        assert_eq!(row.totals.total_tokens, 200);
        assert_eq!(row.totals.estimated_tokens, 100);
        assert_eq!(row.totals.subagent_tokens, 100);
        assert!(row.estimated);
        assert!(!row.external);
        assert_eq!(
            row.model, "model-b",
            "dominant model still considers the whole session"
        );
        assert_eq!(row.headline.as_deref(), Some("Task"));
        assert_eq!(row.headline_raw.as_deref(), Some("Raw task"));
        assert_eq!((row.turns, row.tool_calls, row.retries), (4, 2, 1));

        let all = s.usage_sessions_between(from, to, None, 50).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], filtered[0]);
        assert_eq!(all[1].session_id, "external");
        assert_eq!(all[1].lane_id, None);
        assert!(all[1].external);
        assert_eq!(all[2].totals.events, 3);
        assert_eq!(
            s.usage_sessions_between(from, to, Some(3), 1)
                .await
                .unwrap(),
            filtered[..1]
        );
        assert_eq!(
            s.usage_sessions_between(from, to, None, 1).await.unwrap(),
            all[..1]
        );
        for lane in [None, Some(0), Some(3), Some(999)] {
            assert!(
                s.usage_sessions_between(from, to, lane, 0)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(
                s.usage_sessions_between(to, to, lane, 50)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        for lane in [0, 999] {
            assert!(
                s.usage_sessions_between(from, to, Some(lane), 50)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn a_session_without_a_stored_digest_still_appears_from_its_events() {
        let s = store().await;
        s.record_usage_events(vec![usage_event(
            0,
            "claude-sonnet-5",
            "2026-09-01T10:00:00Z",
        )])
        .await
        .unwrap();
        let from = chrono::DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let rows = s
            .usage_sessions_between(from, from + chrono::Duration::days(1), None, 50)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].headline.is_none());
        assert_eq!(rows[0].turns, 0);
    }

    #[test]
    fn migration_28_indexes_model_activity_on_fresh_and_existing_databases() {
        for existing in [false, true] {
            let mut conn = Connection::open_in_memory().unwrap();
            if existing {
                for (target, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 27) {
                    conn.execute_batch(sql).unwrap();
                    conn.pragma_update(None, "user_version", target).unwrap();
                }
                conn.execute("INSERT INTO usage_events(at, agent_kind, model, account, source_path, source_offset) VALUES ('2026-09-01T00:00:00Z', 'claude-code', 'test-model', 'default', 'fixture', 0)", []).unwrap();
            }
            init(&mut conn).unwrap();

            run_migrations(&mut conn).unwrap();
            let version: i64 = conn
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
            assert!(version >= 28);
            let mut stmt = conn.prepare("EXPLAIN QUERY PLAN SELECT model, MAX(at), SUM(CASE WHEN at >= ?1 THEN input_tokens + output_tokens + cache_read_tokens + cache_write_tokens ELSE 0 END) FROM usage_events GROUP BY model ORDER BY model ASC").unwrap();
            let plan: Vec<String> = stmt
                .query_map(["2026-08-01T00:00:00Z"], |r| r.get(3))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            assert!(
                plan.iter()
                    .any(|line| line.contains("idx_usage_events_model")),
                "{plan:?}"
            );
            assert!(
                !plan.iter().any(|line| line.contains("TEMP B-TREE")),
                "{plan:?}"
            );
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM usage_events", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, i64::from(existing));
        }
    }

    #[tokio::test]
    async fn migration_23_applies_fresh_and_from_22() {
        let dir = tempfile::tempdir().unwrap();
        let mut fresh = Connection::open(dir.path().join("fresh.db")).unwrap();
        init(&mut fresh).unwrap();
        let v: i64 = fresh
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert!(v >= 23);

        let mut staged = Connection::open(dir.path().join("staged.db")).unwrap();
        for (target, sql) in MIGRATIONS.iter().filter(|(v, _)| *v <= 22) {
            let tx = staged.transaction().unwrap();
            tx.execute_batch(sql).unwrap();
            tx.pragma_update(None, "user_version", target).unwrap();
            tx.commit().unwrap();
        }
        assert!(
            staged
                .prepare("SELECT 1 FROM usage_events LIMIT 1")
                .is_err(),
            "usage_events must not exist before migration 23"
        );
        run_migrations(&mut staged).unwrap();
        staged
            .prepare("SELECT 1 FROM usage_events LIMIT 1")
            .expect("usage_events exists after migration 23");
        let v: i64 = staged
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert!(v >= 23);
    }
}
