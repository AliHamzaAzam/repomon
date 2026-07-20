# Phase 2: Orchestration Journal + Cold-Start Recap Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every orchestrator-initiated action lands in a durable, searchable `orchestration_log` SQLite table, and a fresh repomind session opens with a recap instead of re-discovery.

**Architecture:** The MCP layer journals centrally in `Server::call` (one choke point — it alone knows tool semantics and outcomes) through two new daemon RPCs, `journal.append` and `journal.query`. The daemon owns storage (migration 0007). `fleet_history` exposes search/recap as an MCP tool; `fleet_status`'s first call each session embeds a `since_you_last_looked` digest. Session boundaries are `session_start` journal entries appended lazily once per MCP process.

**Tech Stack:** rusqlite (existing hand-rolled migrations, `PRAGMA user_version`), LIKE-substring search (matches `search_commits`; the roadmap's FTS5 aside is wrong about the existing code — deviation noted in PR), serde_json digests.

## Global Constraints

- Branch `feat/orchestration-journal` stacked on `feat/repo-notes`; PR base = `feat/repo-notes`.
- `journal.*` RPCs are local-socket only (remote lock-in test, no production change).
- Journal writes are best-effort from the MCP side: a journal failure must never fail the tool call it records.
- Params/detail digests truncated (300 chars) so the journal can't bloat from embedded notes/transcripts.
- No em-dashes in commit messages.
- TDD: red test before each implementation; `cargo fmt` + clippy clean at every commit.

---

### Task 1: Store layer (migration 0007 + JournalEntry + queries)

**Files:**
- Create: `crates/repomon-core/migrations/0007_orchestration_log.sql`
- Modify: `crates/repomon-core/src/store/mod.rs` (MIGRATIONS array, new fns, tests)
- Modify: `crates/repomon-core/src/model.rs` (JournalEntry struct)

**Interfaces (Produces):**
```rust
// model.rs
pub struct JournalEntry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub session: String,
    pub action: String,
    pub lane_id: Option<i64>,
    pub repo: Option<String>,
    pub params: Option<String>,
    pub outcome: String,          // "ok" | "error"
    pub detail: Option<String>,
}
// store
pub async fn append_journal(&self, e: JournalEntry) -> Result<i64>;              // id ignored on insert
pub async fn recent_journal(&self, limit: usize) -> Result<Vec<JournalEntry>>;   // newest first
pub async fn search_journal(&self, query: String, limit: usize) -> Result<Vec<JournalEntry>>; // LIKE over action/repo/params/detail, newest first
pub async fn journal_since_prev_session(&self, limit: usize) -> Result<Vec<JournalEntry>>;    // entries with id > id of the second-newest 'session_start', ascending; empty when <2 session_starts
```

SQL (0007):
```sql
CREATE TABLE IF NOT EXISTS orchestration_log (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    at      TEXT NOT NULL,
    session TEXT NOT NULL,
    action  TEXT NOT NULL,
    lane_id INTEGER,
    repo    TEXT,
    params  TEXT,
    outcome TEXT NOT NULL,
    detail  TEXT
);
CREATE INDEX IF NOT EXISTS idx_orchestration_log_at ON orchestration_log(at);
CREATE INDEX IF NOT EXISTS idx_orchestration_log_action ON orchestration_log(action);
```

- [ ] Tests first (store tests mod): append+recent round-trip (newest first); search matches params substring case-insensitively; `journal_since_prev_session` with two sessions returns previous session's actions plus later entries ascending and excludes entries before the previous session_start; with one session returns empty.
- [ ] Verify RED, implement, verify GREEN: `cargo test -p repomon-core journal`
- [ ] Commit.

### Task 2: Daemon RPCs `journal.append` / `journal.query`

**Files:**
- Modify: `crates/repomon-daemon/src/rpc.rs` (param structs + dispatch arms near repo.notes.*)
- Modify: `crates/repomon-daemon/tests/integration.rs` (new test `journal_append_and_query`)
- Modify: `crates/repomon-daemon/src/remote.rs` (lock-in: `journal.append`, `journal.query` in blocked list)

**Interfaces (Produces):**
- `journal.append {action, session, lane_id?, repo?, params?, outcome?("ok"), detail?}` → `{id}` (id = rowid)
- `journal.query {query?} | {since_last_session: true} | {} , limit? (default 50, cap 200)` → `{entries: [JournalEntry]}`; search/newest-first when `query`, recap/ascending when `since_last_session`, else recent newest-first.

- [ ] RED integration test: append 2 entries in session "a", 1 in session "b" (session_start rows included), query {} → newest first; {query} filters; {since_last_session} → session-a actions ascending.
- [ ] Implement arms (`store.append_journal` etc.), GREEN: `cargo test -p repomon-daemon --test integration journal`
- [ ] Remote lock-in additions; `cargo test -p repomon-daemon --lib remote_allowlist`
- [ ] Commit.

### Task 3: MCP central journaling + `fleet_history` tool + recap

**Files:**
- Modify: `crates/repomon-mcp/src/server.rs`
- Modify: `crates/repomon-daemon/tests/mcp_stdio.rs`

**Design:**
- `Server` gains `session: String` (pid+nanos, built in `Server::new`) and `session_started: tokio::sync::OnceCell<()>` plus `recap_shown: AtomicBool`.
- `fn journaled_tool(name) -> bool` for: spawn_agent, send_to_agent, approve_agent, interrupt_agent, stop_agent, create_lane, delete_lane, merge_lane, repo_notes_write.
- In `Server::call`: after the handler returns, if `journaled_tool`, `self.journal(name, &args, &out).await` (best-effort): ensures `session_start` appended once (params = autonomy/max_agents), then appends `{action: name, lane_id: args.lane_id, repo: args.repo, params: digest(args, 300), outcome, detail: err or result digest(200)}`.
- `fleet_history` tool (ungated read): args `{query?, since_last_session?, limit?}` → forwards to `journal.query`, maps entries to compact `{at, action, repo, lane_id, outcome, params}`. Catalog 16 tools.
- `fleet_status`: on first call per process (`recap_shown` swap), `ensure_session_started()` then `journal.query {since_last_session}` → `since_you_last_looked: {entries: N, recent: [up to 10 compact lines]}` merged into the result (omitted after first call; `{entries: 0}` still shown on first call).

- [ ] RED mcp_stdio: catalog 16 + `fleet_history` name; two-child e2e: child A does `repo_notes_write` (journaled) then exits; child B `fleet_status` → `since_you_last_looked.recent` mentions repo_notes_write, second `fleet_status` omits the block; `fleet_history {query: "repo_notes_write"}` returns the entry; read-only child can call `fleet_history` (read is ungated).
- [ ] GREEN: `cargo test -p repomon-daemon --test mcp_stdio`
- [ ] Commit.

### Task 4: Persona update + guard

**Files:**
- Modify: `crates/repomon-mcp/assets/repomind.md`
- Modify: `crates/repomon-mcp/src/server.rs` (extend `persona_documents_repo_notes` or add `persona_documents_fleet_history`)

- [ ] RED: persona test asserts PERSONA contains `fleet_history` and `since_you_last_looked`.
- [ ] Edit persona: Orient explains the first `fleet_status` includes `since_you_last_looked` (explain unexplained states via `fleet_history`/`read_agent`); add a short `## History` note: every mutating action is journaled automatically; use `fleet_history` for "what happened with X".
- [ ] GREEN: `cargo test -p repomon-mcp persona`
- [ ] Commit.

### Task 5: Full verification + PR

- [ ] `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [ ] Push `feat/orchestration-journal`, PR with base `feat/repo-notes`; notes: LIKE search not FTS5 (matches search_commits; FTS5 can come later behind the same RPC), journal is MCP-side-scoped (orchestrator actions only; TUI actions are not journaled in this phase), manual acceptance deferred to combined end-of-roadmap run.

## Dependency order
1 → 2 → 3 → 4 → 5.
