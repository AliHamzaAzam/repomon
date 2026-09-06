# Brief R1: Repomind home repo and controller lane (daemon)

Spec: `docs/superpowers/specs/2026-09-04-repomind-home-design.md`. Read it fully; sections 1, 2,
2b, 5 and phase R1 are this brief's contract. R0 (the `~/repomind` template files, git init,
private remote, basic-memory project) is being done in parallel by another agent; do not write
template content yourself, but your ensure-home code must create the same layout when the
folder is missing (use the layout in spec section 1; content of `AGENTS.md`/`REPOMIND.md` can
be a short placeholder that R0's files will replace, and ensure-home must never overwrite files
that already exist).

Branch: `feat/repomind-controller-lane` in a fresh worktree at
`/private/tmp/repomon-feat-repomind-r1` from current main.

## Scope

1. **Config** (`crates/repomon-core/src/config.rs`): a `[repomind]` table with `home`
   (default `~/repomind`, tilde-expanded), `primary_agent` (default: the existing
   `orchestrator_agent` value), `max_controllers` (default 2). `#[serde(default)]` so existing
   configs load unchanged; round-trip tests; surfaced in `config.get` and the desktop
   `ConfigView` type (regenerate bindings).
2. **Lane role** (`crates/repomon-core/src/store/`): migration `00NN_lane_role.sql` (next number
   after the highest existing; never reuse) adding nullable `lanes.role TEXT`; `Lane.role:
   Option<String>` in `model.rs` with ts-rs; store helpers `set_lane_role`, `controller_lane()`.
   Tests: fresh and pinned-previous-version DB migrate; `user_version` bumps.
3. **Ensure home** (`crates/repomon-daemon/src/repomind.rs`, new): on daemon start (after the
   store opens) and on `orchestrator.start`: create `repomind.home` if missing with the spec's
   layout and a `.gitignore` for `.repomind/`; `git init -b main` if not a repo; `repo.add` it if
   not registered (name `repomind`); ensure exactly one lane on its default branch with
   `role = "controller"`. Idempotent; never overwrites existing files; logs one line per action.
   Tests with a tempdir home and a temp store.
4. **Spawn into the lane**: `orchestrator.start` spawns the primary agent into the controller
   lane's window (a normal lane window, not the `orchestrator` tmux window) with cwd = the home
   and `REPOMON_MCP_MODE=orchestrator` (full catalog). `orchestrator.stop/.target/.send_input/
   .key/.resize/.watch` become thin aliases onto the controller lane's primary window, marked
   deprecated in `docs/protocol.md`. `ORCHESTRATOR_WINDOW` consumers (`notify_watch`'s attention
   classification, `stream_orchestrator`, `lib.rs`) resolve the controller window instead.
   Existing orchestrator tests must pass or be updated with the same assertions on the new path.
5. **Controllers and policy**: `agent.spawn` (and the MCP `spawn_agent`) into the controller
   lane produces a controller: full catalog, identity token as usual, `role: "controller"` in
   the session payload (ts-rs), capped by `max_controllers` (refuse with a clear error). A caller
   whose identity is a worker (restricted mode) is refused when targeting the controller lane.
   In `repomon-mcp`'s policy layer: refuse `delete_lane` and `merge_lane` on the controller lane
   outright (before the two-phase confirm). Tests for each refusal.
6. **Remote bridge** (`remote.rs`): `repomind.*` read RPCs allowed, none of the above writes
   beyond what `orchestrator.*` already allowed; update the exclusion list test.
7. **Docs**: `docs/architecture.md` repomind section rewritten for the lane model;
   `docs/protocol.md` for the new fields and deprecations; `docs/desktop.md` a short note that
   Repomind now lives in a lane (the sidebar row and panel come in R4).

Out of scope here: exports to markdown (R2), boot context (R3), the sidebar row and panel (R4).

## Rules and gate

Work only in the worktree; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`. Live verification with the isolation recipe in
`apps/desktop/e2e/isolated.sh` (unique tmux session, throwaway config/data/socket, `default_agent
= "fake"`): after `orchestrator.start`, `lane.list` shows the controller lane with the running
window, a `spawn_agent` from a worker identity into it is refused, and `delete_lane` on it is
refused. No hex colors, no emoji, no em-dashes. Gate: `cargo test -p repomon-core`,
`cargo test -p repomon-daemon`, `cargo test -p repomon-mcp`; in `apps/desktop` `bun run check`,
`bun run test`, `bun run bindings:check`. Commits: one per numbered item where sensible, 1-line
Conventional Commits (`feat(core): ...`, `feat(daemon): ...`, `feat(mcp): ...`), no co-author
trailer. Do not merge, push, or build the bundle. Report commit hashes, gate tails, the live
verification transcript, and anything left out.
