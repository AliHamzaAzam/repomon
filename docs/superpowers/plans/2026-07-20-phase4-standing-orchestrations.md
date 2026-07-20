# Phase 4: Standing Orchestrations (Schedules + Triggers) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** repomind runs without a human at the keyboard: daemon-owned schedules spawn bounded headless orchestrator runs whose results arrive as notifications and journal entries, and a needs-you edge with no UI attached can trigger a bounded triage run that pushes context plus a recommendation.

**Architecture:** A `schedules` table (migration 0009) + a pure spec parser in repomon-core (`daily/weekdays/weekends HH:MM`, `every Nm/Nh`). A new daemon module `standing.rs` owns the scheduler tick and the headless runner: `claude -p <prompt>` with the existing MCP-config file mechanism plus `REPOMON_MCP_UNATTENDED=1` and a lower action cap, wall-clock-limited, output journaled (`action=standing_run`) and delivered through the existing `push::send_all` + `notify::send_native` paths. The MCP policy hard-refuses `merge_lane`/`delete_lane` when unattended. The needs-you triage trigger lives in notify_watch, config-gated (off by default) behind `triage_after_mins`.

**Tech Stack:** existing crates only; tokio::process with timeout for the runner; Claude backend only for headless runs (codex `exec` deferred).

## Global Constraints

- Branch `feat/standing-orchestrations` stacked on `feat/playbooks`; PR base = `feat/playbooks`.
- Unattended runs are MORE conservative than attended (locked decision): `REPOMON_MCP_UNATTENDED=1` refuses `merge_lane` + `delete_lane` outright regardless of autonomy; default max_actions for standing runs = 10 (schedule.add accepts an explicit value, capped at 50); wall-clock limit `standing_timeout_secs` config, default 600.
- `schedule.*` RPCs local-only (remote lock-in).
- Triage trigger is config-gated OFF by default (`triage_after_mins` absent = disabled) — an unattended run costs real tokens and must be opted into.
- Headless runner supports the Claude backend only; a codex orchestrator_agent errors at schedule.add time with a clear message.
- No em-dashes in commit messages; TDD red-first; fmt/clippy clean per commit.

---

### Task 1: Schedule spec parser (repomon-core, pure)

**Files:** Create `crates/repomon-core/src/schedule.rs`; register in `lib.rs`.

**Produces:**
```rust
pub enum Spec { Daily { h: u32, m: u32 }, Weekdays { h: u32, m: u32 }, Weekends { h: u32, m: u32 }, Every { minutes: i64 } }
pub fn parse_spec(s: &str) -> Result<Spec>;               // Error::Config with examples on bad input
impl Spec { pub fn next_after(&self, after: DateTime<Local>) -> DateTime<Local>; pub fn canonical(&self) -> String; }
```

- [ ] Tests RED: `"daily 09:00"`, `"weekdays 9:00"`, `"weekends 21:30"`, `"every 30m"`, `"every 2h"` parse; `"tuesdays"` / `"daily 25:00"` / `"every 0m"` rejected with example-bearing errors; `next_after` from a Friday 10:00 for `weekdays 09:00` → Monday 09:00; daily rolls to tomorrow when past; `every 30m` → after+30m.
- [ ] GREEN: `cargo test -p repomon-core schedule`; commit.

### Task 2: Schedules store (migration 0009)

**Files:** `crates/repomon-core/migrations/0009_schedules.sql`, `store/mod.rs`, `model.rs`.

```sql
CREATE TABLE IF NOT EXISTS schedules (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    spec        TEXT NOT NULL,
    prompt      TEXT NOT NULL,
    max_actions INTEGER NOT NULL,
    created_at  TEXT NOT NULL,
    last_run_at TEXT
);
```
**Produces:** `Schedule {id, spec, prompt, max_actions, created_at, last_run_at}`; `add_schedule(spec, prompt, max_actions) -> Schedule`; `list_schedules()`; `remove_schedule(id)` (NotFound on miss); `mark_schedule_run(id, at)`.

- [ ] Tests RED → GREEN: `cargo test -p repomon-core --lib schedule`; commit (with Task 1 if convenient).

### Task 3: Daemon `schedule.add/list/remove` RPCs + lock-in

**Files:** `rpc.rs`, `tests/integration.rs`, `remote.rs`.

- `schedule.add {spec, prompt, max_actions?}`: `parse_spec` validates (invalid_params with examples); prompt non-empty, ≤2000 bytes; max_actions default 10, cap 50; errors if the resolved orchestrator backend is Codex ("headless standing runs support the claude backend only"). Returns the row + `next_run` (computed).
- `schedule.list {}` → `{schedules: [row + next_run]}`; `schedule.remove {id}`.
- All three in the remote blocked list.

- [ ] RED integration test `schedule_add_list_remove`: add valid → listed with next_run; bad spec rejected (error mentions "daily"); empty prompt rejected; remove works, second remove errors.
- [ ] GREEN + lock-in; commit.

### Task 4: Unattended MCP policy

**Files:** `crates/repomon-mcp/src/policy.rs`, `src/server.rs`, `crates/repomon-daemon/tests/mcp_stdio.rs`.

- `Policy` gains `pub unattended: bool` from `REPOMON_MCP_UNATTENDED` (`"1"`/`"true"`).
- `merge_lane` and `delete_lane` handlers refuse first when `policy.unattended`: "this is an unattended standing run: merging/deleting is never allowed here. Report the state and recommend the action for the human instead."
- `repomon_mcp::UNATTENDED_ADDENDUM` const (persona addendum for headless runs): bounded run, no merge/delete, report-and-recommend, end with a compact briefing the human can read on a phone.

- [ ] RED: policy unit test (`unattended_from_env` construction helper test), mcp_stdio `mcp_stdio_unattended_refuses_merge`: child with `REPOMON_MCP_UNATTENDED=1` + autonomous → `merge_lane` refused mentioning "unattended", `spawn_agent` still permitted (only caps bound it); persona-side test that `UNATTENDED_ADDENDUM` mentions merge_lane and recommend.
- [ ] GREEN; commit.

### Task 5: Headless runner + scheduler loop (daemon `standing.rs`)

**Files:** Create `crates/repomon-daemon/src/standing.rs`; wire in `lib.rs` (module) + `main.rs` (spawn task); `Config` gains `standing_timeout_secs: u64` default 600 (config.rs + default test).

**Produces:**
```rust
/// Build the headless claude command for a standing run (claude backend only).
pub fn build_headless_command(base: &str, mcp_path: &Path, model: &Option<String>, prompt: &str) -> String; // claude -p, persona+UNATTENDED_ADDENDUM, allowedTools, no session-id
/// Run a command via sh -c with a wall clock; returns (ok, combined-output tail 4000).
pub async fn run_bounded(command: &str, timeout: Duration) -> (bool, String);
/// One scheduler pass: fire every due schedule (mark_schedule_run FIRST, then run, journal, notify).
pub async fn scheduler_tick(ctx: &Arc<Ctx>) ;
pub async fn standing_watch(ctx: Arc<Ctx>);  // 30s interval loop calling scheduler_tick
```
- Journal: `session = "standing-<id>-<nanos>"`, `action = "standing_run"`, params = `{schedule_id, spec, prompt: digest 200}`, outcome ok/error, detail = output tail 4000 (direct `store.append_journal`, no RPC hop).
- Notify: title `"repomind: <first 40 chars of prompt>"`, body = output tail 300; `push::send_all(..., CATEGORY_ALERT, payload)` when `cfg.remote.enabled`, `notify::send_native` when the TUI heartbeat is stale (same `LOCAL_TTL` gating as notify_watch — reuse `ctx.local_watcher_seen`).
- The MCP config for headless runs is written per-run to `config_dir()/repomind-standing-mcp.json` via a parameterized variant of `write_orchestrator_mcp_config` (extra env: `REPOMON_MCP_UNATTENDED=1`, `REPOMON_MCP_MAX_ACTIONS=<n>`); refactor the existing fn to take the extra env pairs (existing call sites unchanged in behavior).

- [ ] RED (integration test in `tests/integration.rs` or new `tests/standing.rs`): `run_bounded("echo hello && echo err >&2", 5s)` → (true, contains both); `run_bounded("sleep 30", 1s)` → (false, mentions timeout); scheduler e2e with a fake orchestrator agent: Ctx with config `agents.insert("noop", "printf 'BRIEFING: all quiet'")`, `orchestrator_agent = Some("noop")`, add schedule `every 1m` backdated last_run... simpler: call `scheduler_tick` directly after inserting a schedule whose `last_run_at` is NULL and spec `every 1m` with created_at backdated (test-only `backdate_schedule` store fn) → journal gains a `standing_run` entry whose detail contains "BRIEFING"; a second immediate tick fires nothing (last_run_at now fresh).
  - NOTE: `build_headless_command` is bypassed for a custom (non-claude) agent command: like `orchestrator_base_command`, a custom agent string is used verbatim with the prompt appended; the claude flags are only added for the claude backend. This is what makes the fake-agent test (and the end acceptance with a real claude) share one code path.
- [ ] GREEN: `cargo test -p repomon-daemon standing`; wire `standing_watch` into main.rs beside `notify_watch`; commit.

### Task 6: Needs-you triage trigger (config-gated)

**Files:** `notify_watch.rs`, `config.rs` (`triage_after_mins: Option<u64>`, default None), `standing.rs` (reuse runner).

- Pure fn + tests: `triage_due(fired: Instant, now: Instant, after_mins: u64, ui_attached: bool) -> bool`.
- In notify_watch: when a NeedsYou fires and `cfg.triage_after_mins = Some(n)`, record `(lane_id, Instant)` in a local pending map. Each tick: entries older than n minutes where the lane still needs attention and still no UI (`!tui_active && ctx.sessions` empty) → remove from map, spawn one triage run (tokio::spawn, standing runner) with prompt: "Triage lane <id> (repo <name>): use read_agent to see its state, classify the situation, and recommend exactly one next action. Do not approve, merge, or delete anything. End with a 2-3 sentence briefing." max_actions 5, journal `action = "triage_run"` with lane_id, notify with the output (CATEGORY_ALERT).
- One triage per (lane, needs-you edge): the pending map entry is consumed; the notify latch prevents re-adds until real activity.

- [ ] RED: unit tests for `triage_due`; integration is covered by the runner tests + end acceptance (spawning a real triage e2e needs a needs-you agent — deferred).
- [ ] GREEN; commit.

### Task 7: CLI (`repomon orchestrate --schedule` + `repomon schedules`)

**Files:** `crates/repomon-tui/src/cli.rs`.

- `Orchestrate` gains `--schedule <spec>` and `--max-actions <n>`: when `--schedule` is present, require a prompt, call `schedule.add`, print the row + next run, exit (no attach).
- New `Command::Schedules { cmd: SchedulesCmd }`: `list` (id, spec, next run, prompt digest), `remove <id>`.
- [ ] Build clean + clippy; commit. (CLI output smoke rides the end acceptance run.)

### Task 8: Persona guard for the addendum + full verification + PR

- [ ] `UNATTENDED_ADDENDUM` guard test lives in Task 4; re-verify.
- [ ] `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- [ ] Push `feat/standing-orchestrations`; PR base `feat/playbooks`. Notes: triage off by default; headless = claude backend only; Phase 5 will let triage answer routine permissions within policy; acceptance deferred to combined run.

## Dependency order
1 → 2 → 3 → 4 → 5 → 6 → 7 → 8.
