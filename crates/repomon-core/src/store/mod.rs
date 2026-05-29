//! The SQLite store.
//!
//! All database access is serialized onto a single dedicated thread that owns the
//! `Connection`. Callers submit closures and await the result over a oneshot channel, so
//! the tokio runtime is never blocked and rusqlite's `!Sync` connection never crosses an
//! `.await`. Schema migrations are hand-rolled against `PRAGMA user_version`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::thread;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, Row};

use crate::error::{Error, Result};
use crate::model::*;

/// Embedded migrations, applied in order. The index + 1 is the target `user_version`.
const MIGRATIONS: &[&str] = &[
    include_str!("../../migrations/0001_init.sql"),
    include_str!("../../migrations/0002_agent_kind.sql"),
];

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

    // ---- repos ---------------------------------------------------------------

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
            })
        })
        .await
    }

    pub async fn list_repos(&self) -> Result<Vec<Repo>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, path, name, added_at, worktree_root_template FROM repos ORDER BY name",
            )?;
            let rows = stmt.query_map([], repo_from_row)?;
            collect(rows)
        })
        .await
    }

    pub async fn get_repo(&self, id: RepoId) -> Result<Repo> {
        self.call(move |c| {
            c.query_row(
                "SELECT id, path, name, added_at, worktree_root_template FROM repos WHERE id = ?1",
                params![id],
                repo_from_row,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Error::NotFound(format!("repo {id}")),
                other => other.into(),
            })
        })
        .await
    }

    pub async fn find_repo_by_path(&self, path: PathBuf) -> Result<Option<Repo>> {
        self.call(move |c| {
            let r = c
                .query_row(
                    "SELECT id, path, name, added_at, worktree_root_template FROM repos WHERE path = ?1",
                    params![path.to_string_lossy()],
                    repo_from_row,
                )
                .map(Some);
            match r {
                Ok(v) => Ok(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e.into()),
            }
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

    // ---- worktrees -----------------------------------------------------------

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

    // ---- lanes ---------------------------------------------------------------

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

    pub async fn list_lane_meta(&self) -> Result<Vec<LaneMeta>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT id, repo_id, worktree_path, pinned, tmux_window, agent_kind FROM lanes",
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

    // ---- commits -------------------------------------------------------------

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

    // ---- agent sessions ------------------------------------------------------

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
}

// ---- connection init + migrations --------------------------------------------

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
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let target = (i + 1) as i64;
        if current < target {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", target)?;
            tx.commit()?;
        }
    }
    Ok(())
}

// ---- row mapping helpers -----------------------------------------------------

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

fn repo_from_row(r: &Row) -> rusqlite::Result<Repo> {
    Ok(Repo {
        id: r.get(0)?,
        path: PathBuf::from(r.get::<_, String>(1)?),
        name: r.get(2)?,
        added_at: dt_col(r, 3)?,
        worktree_root_template: r.get(4)?,
    })
}

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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert!(s
            .find_repo_by_path(PathBuf::from("/code/a"))
            .await
            .unwrap()
            .is_some());
        assert!(s
            .find_repo_by_path(PathBuf::from("/code/z"))
            .await
            .unwrap()
            .is_none());

        s.remove_repo(r.id).await.unwrap();
        assert_eq!(s.list_repos().await.unwrap().len(), 1);
        assert!(matches!(s.get_repo(r.id).await, Err(Error::NotFound(_))));
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

        // Upsert again updates head in place, doesn't duplicate.
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
        // Re-inserting the same oids adds nothing.
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

        // repo filter that excludes everything.
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
}
