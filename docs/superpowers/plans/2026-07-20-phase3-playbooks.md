# Phase 3: Playbooks (Procedural Learning) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** repomind drafts playbooks after completed goals; the human approves them; future sessions search and follow approved playbooks only.

**Architecture:** SQLite table `playbooks` (migration 0008) with an approval-gated lifecycle: `playbook_save` always produces a draft (a revision when the name is already approved, stored in `draft_content` beside the live approved text); `playbook_search` returns approved content only. Approval is a CLI verb (`repomon playbooks approve`); TUI view deferred like the notes view. Unreviewed drafts expire after 30 days via opportunistic sweep.

**Tech Stack:** rusqlite migration + store fns, daemon RPCs `playbook.save/search/list/approve/delete` (all local-only), MCP tools `playbook_save`/`playbook_search` (catalog 18), clap subcommand in repomon-tui.

## Global Constraints

- Branch `feat/playbooks` stacked on `feat/orchestration-journal`; PR base = `feat/orchestration-journal`.
- Approval gate is absolute: `playbook_search` NEVER returns draft content (self-poisoning-prompt defense, locked roadmap decision).
- Content cap 16384 bytes (playbooks ride in prompts); name: 1-64 chars of `[A-Za-z0-9._-]`, validated daemon-side.
- Draft expiry: `DRAFT_TTL_DAYS = 30`, swept opportunistically on save/list.
- `playbook_save` is a journaled, `record_mutation()`-gated mutating tool; `playbook_search` is an ungated read.
- No em-dashes in commit messages. TDD red-first; fmt + clippy clean per commit.

---

### Task 1: Store layer (migration 0008 + Playbook + lifecycle fns)

**Files:** `crates/repomon-core/migrations/0008_playbooks.sql`, `crates/repomon-core/src/store/mod.rs`, `crates/repomon-core/src/model.rs`

**Produces:**
```rust
pub struct Playbook {
    pub name: String,
    pub content: String,            // approved text once approved; draft text before
    pub status: String,             // "draft" | "approved"
    pub draft_content: Option<String>, // pending revision when status == "approved"
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
}
pub async fn save_playbook(&self, name: String, content: String) -> Result<Playbook>; // insert draft | update draft | stash revision on approved
pub async fn search_playbooks(&self, query: String, limit: usize) -> Result<Vec<Playbook>>; // APPROVED ONLY, LIKE name+content
pub async fn list_playbooks(&self) -> Result<Vec<Playbook>>;      // all (post-sweep), name order
pub async fn approve_playbook(&self, name: String) -> Result<Playbook>; // draft->approved; revision->promote
pub async fn delete_playbook(&self, name: String) -> Result<()>;
```
Sweep (`DELETE FROM playbooks WHERE status='draft' AND updated_at < now-30d`) runs inside save/list.

SQL:
```sql
CREATE TABLE IF NOT EXISTS playbooks (
    name          TEXT PRIMARY KEY,
    content       TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'draft',
    draft_content TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    approved_at   TEXT
);
```

- [ ] Tests RED: save creates draft; search hides drafts; approve makes it searchable; save over approved stashes revision (search still returns OLD content); approve promotes revision; expired draft (backdate updated_at via direct SQL exec through a save+manual update helper... use `save_playbook` then `store.call`-free approach: add test-only `#[cfg(test)] pub async fn backdate_playbook(&self, name, days)`) is swept on list; delete removes.
- [ ] Verify RED → implement → GREEN: `cargo test -p repomon-core playbook`
- [ ] Commit.

### Task 2: Daemon RPCs + remote lock-in

**Files:** `crates/repomon-daemon/src/rpc.rs`, `tests/integration.rs`, `src/remote.rs`

- `playbook.save {name, content}` → playbook json. Validates name (1-64, `[A-Za-z0-9._-]`) and content cap 16384 with invalid_params naming the limits.
- `playbook.search {query, limit?}` → {playbooks: [{name, content, approved_at}]} (approved only).
- `playbook.list {}` → {playbooks: [full rows minus content bodies? keep full]}.
- `playbook.approve {name}` / `playbook.delete {name}` → playbook / null. NotFound → invalid_params.
- All five in the remote blocked list.

- [ ] RED integration test `playbook_lifecycle`: save → search empty → approve → search hits → save revision → search still old → approve → search new; bad name rejected ("64"); oversized rejected ("16384").
- [ ] GREEN: `cargo test -p repomon-daemon --test integration playbook`; remote: `cargo test -p repomon-daemon --lib remote_allowlist`
- [ ] Commit.

### Task 3: MCP tools `playbook_save` / `playbook_search`

**Files:** `crates/repomon-mcp/src/server.rs`, `crates/repomon-daemon/tests/mcp_stdio.rs`

- `playbook_save {name, content}`: `record_mutation()?` then `playbook.save`; result `{ok, name, status, hint: "draft until a human approves it: repomon playbooks approve <name>"}`. Added to `journaled_tool`.
- `playbook_search {query}`: ungated; `playbook.search`; `{playbooks: [{name, content, approved_at}]}` + `hint` when empty ("no approved playbook matches; plan from scratch and draft one with playbook_save when the goal completes").
- Catalog 16 → 18; descriptions teach the draft/approve cycle explicitly.

- [ ] RED mcp_stdio `mcp_stdio_playbook_draft_approval_flow`: catalog 18; save → search returns nothing + hint; approve via a direct daemon `playbook.approve` control call; search now returns it; read-only child: save refused, search allowed.
- [ ] GREEN: `cargo test -p repomon-daemon --test mcp_stdio playbook`
- [ ] Commit.

### Task 4: CLI `repomon playbooks list|show|approve|delete`

**Files:** `crates/repomon-tui/src/cli.rs`

- New `Command::Playbooks { cmd: PlaybooksCmd }`; subcommands map 1:1 to the RPCs over the existing client (`crate::ensure_daemon` not needed: use the same connect path as `handle_lane`; look at how `Lane { cmd }` gets a client). `list` prints `name  status  updated_at  (+pending revision)` rows; `show <name>` prints content (and pending revision if any); `approve <name>` prints confirmation; `delete <name>` prints confirmation.
- No automated e2e for CLI output (matches existing CLI verbs which are untested); covered by the daemon integration test + end acceptance run.

- [ ] Implement; `cargo build -p repomon-tui` clean; manual smoke deferred to end run.
- [ ] Commit.

### Task 5: Persona + guard test

**Files:** `crates/repomon-mcp/assets/repomind.md`, `crates/repomon-mcp/src/server.rs`

- Persona: new `## Playbooks (procedural memory)` section: before multi-lane/multi-step goals, `playbook_search`; follow an approved playbook when one matches and tell the human which one; after a goal completes (lanes merged/closed), draft one with `playbook_save` (goal pattern, per-repo steps, worker prompts that worked, verification, failure modes); drafts are inert until the human approves (`repomon playbooks approve`); when reality deviates from an approved playbook, save a revised draft rather than silently diverging.
- Decide step: mention checking playbooks for multi-lane goals.

- [ ] RED: `persona_documents_playbooks` asserts PERSONA contains `playbook_search` and `playbook_save`.
- [ ] GREEN: `cargo test -p repomon-mcp persona`
- [ ] Commit.

### Task 6: Full verification + PR

- [ ] `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [ ] Push `feat/playbooks`; PR base `feat/orchestration-journal`. Notes: TUI approval view deferred (CLI is the approval surface), draft expiry is sweep-on-touch not a background job, acceptance deferred to combined run.

## Dependency order
1 → 2 → 3 → {4, 5} → 6.
