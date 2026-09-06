# Brief R6: Repomind panel redesign (control room, not a second chat)

Context: Repomind now runs as a controller lane (R1 to R5, all on main). The operator started it
live and it works. The right-rail Repomind panel still carries views from the hidden-window era
(composer, Live Feed, Transcript) that duplicate the terminal pane, while the new Home view is
thin. Decision (operator, 2026-09-04): the panel becomes the control room for Repomind's memory
and duties, and the pane stays the conversation.

Branch: `feat/repomind-control-room` in a fresh worktree at `/private/tmp/repomon-feat-repomind-r6`
from current main. Frontend in `apps/desktop` plus small daemon additions. Run `/frontend-design`
and `/impeccable` before touching UI and follow them; the panel must belong to the same visual
system as the Repomail and Supervision panels and the new sidebar rows.

## Scope

1. **Remove the duplicates.** Drop the panel's composer ("Coordinate the fleet..." and Send
   Instruction), the Live Feed view, and the Transcript view. Clicking the pinned row or a
   controller in the panel focuses that controller's pane in the terminal bay (already wired for
   the row; add it for the controller list). Keep `mod+5` as the panel toggle. Remove the dead
   store code and tests those views owned; keep the `repomind.status` polling store.
2. **Plans board.** Section listing `plans/active/*.md` (title, owner, next step, updated),
   an "Add goal" control (title plus one line of intent) that writes
   `plans/active/<slug>.md` with the home's frontmatter conventions through the existing
   `file.write` on the home lane and then sends the primary controller a verified-line
   instruction "New goal in plans/active/<slug>.md: <title>. Pick it up." (reuse the daemon's
   verified injection through a small local-only `repomind.instruct { text }` RPC that targets the
   primary controller window and refuses when none is running), and a "Done" action per plan
   that prompts for a one-line outcome and moves the file to `plans/done/` via `file.rename`
   plus an appended outcome line. Empty state copy explains the file shape. Tests with mocked
   RPCs.
3. **Playbooks.** Section with two lists: drafts (`playbooks/drafts/*.md`) each with Approve and
   Reject, and approved (`playbooks/*.md`) each with an open-in-editor link. Approve calls the
   existing `playbook.approve`; add a local-only `playbook.reject { name }` RPC in the daemon
   that moves the draft to `playbooks/rejected/<name>.md` with `status: rejected` (never
   deletes), documented in `docs/protocol.md`, covered by a daemon test and the export commit
   path. Tests.
4. **Standing duties.** Section listing schedules from the existing `schedule.list` (spec, goal,
   cap, last run, next run) with Remove (confirm) using `schedule.remove`; "Add" opens the
   existing Settings > Automation > Schedules surface rather than duplicating its form. Tests.
5. **Memory health.** Section with boot context (tokens, trimmed list, Regenerate, Open
   boot.md), export state (last run, pending, error, Export now), and a journal browser: a
   day picker over `journal/*.md` (newest first, archive months listed under a disclosure)
   showing the selected day's entries, each with an open-in-editor link. Tests.
6. **Controllers.** Keep the list with status pills and reasons; add Focus pane, and keep Start,
   Stop, Spawn in the header with the cap. The header shows the lane, home path, and controller
   count.
7. **Controller status semantics.** In the fleet store, a controller agent whose status is
   `waiting` because its turn ended (no pending dialog, no question) reads IDLE, not NEEDS YOU;
   NEEDS YOU is reserved for a pending dialog or an explicit question (the daemon already
   classifies `end_of_turn` versus dialog attention for the orchestrator in `notify_watch`;
   expose that distinction on the session payload as `attention_kind` if it is not already
   there, ts-rs, bindings regenerated). The pinned row, the toolbar dot, and the panel all use
   the same derivation. Tests: end-of-turn controller shows IDLE; a dialog shows NEEDS YOU; a
   worker's mapping is unchanged.
8. **Docs.** `docs/desktop.md` Repomind section for the new panel sections; `docs/protocol.md`
   for `repomind.instruct` and `playbook.reject`.

## Rules and gate

Work only in the worktree; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`; tests use mocked RPCs and tempdir homes (with the
`BASIC_MEMORY_CONFIG_DIR` override), never the operator's real `~/repomind`. Zero hex color
literals, no `<Show keyed>` around anything expensive, async results token guarded, no emoji
(SVG icons), no em-dashes in code, comments, or copy. Gate: `bun run check`, `bun run test`,
`bun run bindings:check` in `apps/desktop`; `cargo test -p repomon-daemon` and `-p repomon-core`
for the daemon additions. Commits: 1-line Conventional Commits per numbered item where
sensible, no co-author trailer. Do not merge, push, or build the bundle. Report commit hashes,
per-item summary, test names, gate tails, and anything left out with the reason.
