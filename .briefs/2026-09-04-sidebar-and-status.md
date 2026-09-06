# Brief K: Fleet sidebar redesign and agent status correctness

Two parts, in order, on one branch: `feat/fleet-sidebar-status` in a fresh worktree at
`/private/tmp/repomon-feat-fleet-sidebar` from current main (1355bb5 or later). Part 1 is a
correctness investigation and fix (daemon and store); part 2 is a design pass on the sidebar.
Commit them separately (part 1 may need several commits).

## Part 1: agent status is mostly wrong

The operator reports that the statuses shown in the sidebar (RUNNING, IDLE, NEEDS YOU, the
"Needs attention" and "Running" filter counts, the per-lane pills) are mostly incorrect. In the
screenshot the Upwork lane shows "2 RUNNING" while the Running filter shows 1; several lanes
with live agents show no status at all.

Work evidence-first, no guessing:

1. **Collect ground truth now.** For every lane and agent window in the live fleet, record side
   by side: the daemon's view (`repomon --socket /tmp/repomon-azaleas.sock` CLI if it exposes
   lane/agent listing, otherwise a small script speaking JSON-RPC over the socket: `lane.list`
   and any `agent.*` status fields; read-only calls only) and the actual pane state
   (`tmux -L repomon capture-pane -p -t <window>` last 15 lines; use `tmux -L repomon
   list-windows -a` to enumerate). Classify the true state by eye: generating (spinner or
   streaming output), idle at prompt, permission or question dialog pending, exited or shell,
   external session. Tabulate mismatches in `qa/status-audit.md` in the worktree (untracked).
   Do this for at least 8 windows including the Antigravity window `lane-1`, the Codex windows,
   and the Claude Code windows on lane 81.
2. **Trace the pipeline.** Daemon side: how `status`, `attention`, `idle_secs`, `stalled_since`,
   and the prompt classification are computed (`crates/repomon-core/src/agent/` including
   `prompt.rs`'s `detect_dialog`, activity tracking, `notify_watch.rs`, the `lane.list` and
   `fleet_status` payloads, `supervision.rs`'s idle detection). Frontend side: how
   `apps/desktop/src/stores/fleet.ts` derives the row labels, the pills, and the filter counts
   from those fields (look for the `Attention`, `status`, `running`, `needsYou` derivations and
   any client-side timeouts). Find where the ground truth diverges and why: stale
   `last_activity_at`, spinner glyphs not recognised for a given CLI version (Antigravity 1.1.x,
   Codex, Claude Code 2.1.x), dialog detection false positives or negatives, per-connection
   viewport filtering hiding updates for lanes not in view, counts computed over a different
   set than the rows, and so on.
3. **Fix at the source** with fixtures: add real pane captures from step 1 as test fixtures
   (redact anything private) so each mismatch becomes a failing test first. Make the frontend a
   pure function of daemon fields (no client-side re-interpretation of pane text). Make the
   filter counts and the pills come from the same derivation. Add an explainable `status_reason`
   (or similar) string to the agent payload (behind ts-rs, regenerate bindings) so a tooltip on
   the pill can say why ("spinner seen 2s ago", "permission dialog: Bash", "no output for 6m").
4. **Definitions** to align to (document them in `docs/desktop.md` under Fleet):
   running = output changed within the last 10 s or a known spinner/streaming marker is on
   screen; needs you = a permission, decision, or question dialog is pending, or the agent asked
   a question and is waiting; idle = at its prompt with no pending dialog; stalled = running
   state older than the stall threshold with no change; exited = process gone or a bare shell;
   external = session not managed by the daemon. Counts: "Running" counts agents (not lanes)
   in running; "Needs attention" counts agents in needs you or stalled; a lane pill shows the
   most urgent state among its agents plus the agent count.
5. Re-run the audit from step 1 against your patched daemon in an isolated environment where
   possible (the recipe in `apps/desktop/e2e/isolated.sh`; never bind `/tmp/repomon-azaleas.sock`,
   never copy the production DB) and, for the live fleet, at minimum re-run the read-only
   comparison using a debug build of the CLI pointed at the production socket (read-only calls
   only). Put the before/after table in the report.

## Part 2: sidebar design pass (`apps/desktop/src/components/` fleet sidebar files, `index.css`)

Run `/frontend-design` and `/impeccable` first and audit the current sidebar against the
screenshot. Concrete problems to solve:

- The "Hidden (9)" section spends nine full rows with an "Unhide" link each and dominates the
  sidebar. Collapse it to a single compact row ("9 hidden" with a disclosure) that expands to
  a dense list; unhide via a small icon button or the row's context menu.
- Lane rows are inconsistent: the primary lane row is heavy (agent-count pill, status pill,
  diff counts) while worktree lanes are bare branch names with no agent or status info even
  when they have agents. Every lane row should show the same compact structure: name, branch,
  agent count, the most urgent status, and the change indicator, sized so 12 lanes fit without
  scrolling at the default sidebar height.
- Repo header counts ("REPOMON 9") are unexplained; show what the number is (lanes) via label or
  tooltip, and give the header a subtle needs-you roll-up.
- The filter chips ("Needs attention 0", "Running 1") must agree with the rows (part 1) and
  read as toggles; consider a third chip for "Idle".
- Lanes whose branch is already merged into the repo's default branch (all the
  `repomon-feat-editor-*` worktrees in the screenshot) should carry a quiet "merged" marker so
  stale worktrees are obvious; a context-menu action "Remove worktree" may exist already, wire
  it if so, do not add destructive actions without a confirm.
- Typography and spacing: reduce row height, align the change indicators in one column, keep
  the uppercase monospace repo labels but tone the tracking down, keep the rate-limits card.
- Keyboard: arrow navigation across lanes and Enter to focus a lane's agent should keep working;
  verify with the existing tests.

No emoji glyphs (SVG icons only), no em-dashes in copy, colors only through the app's CSS
variables. Screenshots are best-effort (macOS screen-recording permission usually blocks them
from an agent shell); if they fail, say so and rely on tests plus the DOM structure.

## Gate

`bun run check`, `bun run test`, `bun run bindings:check` in `apps/desktop`;
`cargo test -p repomon-core` and `cargo test -p repomon-daemon` at the worktree root. Rules:
work only in the worktree, never touch the main checkout, `/Applications/Repomon.app`, or the
production socket except for read-only status calls in part 1; never kill processes by name
pattern; never `git add -A`. Commits: 1-line Conventional Commits (`fix(daemon): ...`,
`fix(desktop): ...`, `feat(desktop): ...`), no co-author trailer. Do not merge, push, or build
the bundle. Report with the status audit tables (before and after), root causes found, commit
hashes, gate tails, and anything left out.
