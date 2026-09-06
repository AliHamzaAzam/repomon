# Brief R2: Repomind exports, file-first records, basic-memory

Spec: `docs/superpowers/specs/2026-09-04-repomind-home-design.md`, sections "Relationship to
the July plan", 1, 3, and phase R2. R1 is merged (config `[repomind]`, `lanes.role`,
`crates/repomon-daemon/src/repomind.rs` ensure-home, controller lane, `repomind.status`).
The operator's real home exists at `~/repomind` (R0, commit eb50de7, private remote
`AliHamzaAzam/repomind`, basic-memory project `repomind` already registered).

Branch: `feat/repomind-exports` in a fresh worktree at `/private/tmp/repomon-feat-repomind-r2`
from current main.

## Scope

1. **Export module** (`crates/repomon-daemon/src/repomind/export.rs` or a sibling file): one-way,
   idempotent, debounced 5 s, triggered by store writes to the journal, schedules, and approval
   rules, and by a new local-only `repomind.export` RPC. Targets in the home:
   - `journal/YYYY-MM-DD.md`: one section per journal row, keyed by row id in an HTML comment
     or frontmatter list so re-runs append only new rows; day file frontmatter with `title`,
     `type: journal`, `permalink`, `source: repomond`.
   - `plans/standing/<slug>.md`: one file per schedule (spec, goal, cap, last run, next run).
   - `profile/approvals.md`: the approval rules grouped by repo.
   Export state (last exported ids, mtimes) lives in `.repomind/export.json`. Never touch files
   outside those targets. Tests with a tempdir home: append-only journal, idempotent re-run,
   schedule add/remove reflected, approval rules rendered.
2. **File-first repo notes**: the `repo_notes`/`repo_notes_write` MCP tools and the daemon's repo
   notes RPCs read and write `fleet/<repo>/notes.md` in the home (frontmatter plus body). One-time
   migration on daemon start: existing notes in the store or app-support dir are written to the
   home if the file does not exist, then the old location is left read-only (do not delete).
   `create_lane`/`spawn_agent` keep embedding the notes. Tests.
3. **File-first playbooks**: `playbook_save` writes `playbooks/drafts/<slug>.md` (frontmatter:
   `title`, `status: draft`, `source`, `created`, `revises` when it targets an approved name);
   approval (the existing CLI `repomon playbooks` and the desktop panel flow) moves the file to
   `playbooks/<slug>.md` with `status: approved`; `playbook_search` reads approved files only.
   One-time migration of existing store rows into files on start (drafts to `drafts/`, approved
   to the root). Tests: draft never visible to search, approval moves it, revision of an approved
   name lands as a draft beside it.
4. **Commits by the daemon**: after each export batch, `git add` the touched export targets only
   and commit in the home as author `Repomind <repomind@local>` with
   `chore(repomind): export <journal|schedules|approvals|notes|playbooks>`; batch at most once
   per minute; never push; never touch untracked operator files. Skip silently if the home is
   not a git repo. Test with a tempdir repo: exactly one commit per batch, correct author.
5. **basic-memory registration**: on daemon start, if the `basic-memory` CLI is on PATH and
   `~/.basic-memory/config.json` lacks a `repomind` project, run
   `basic-memory project add repomind <home>`; never change the default project, never remove
   projects; log one line. Test by pointing at a temp config file (or by mocking the command).
6. **Panel plumbing for R4**: `repomind.status` gains `export: { last_run, pending, last_error }`
   and `counts: { active_plans, standing, playbooks, drafts }` read from the home. ts-rs type,
   bindings regenerated, `docs/protocol.md`.
7. **Pre-existing breakage**: `cargo test -p repomon-tui` does not compile on main (fixtures
   missing `AgentSession.status_reason` and `WorktreeState.merged`). Fix the fixtures so the
   whole workspace test suite compiles; add `cargo test -p repomon-tui` to your gate.

Out of scope: boot context (R3), the sidebar row and panel UI (R4).

## Rules and gate

Work only in the worktree; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`; never write to the operator's real `~/repomind` from tests (use a
tempdir home in every test and in the isolated live run). No emoji, no em-dashes anywhere.
Gate: `cargo test -p repomon-core`, `-p repomon-daemon`, `-p repomon-mcp`, `-p repomon-tui`;
in `apps/desktop` `bun run check`, `bun run test`, `bun run bindings:check`. Live check with the
isolated recipe: start, append a journal row through an MCP action, see the day file appear
and one commit land; save a playbook draft, approve it via the CLI, see it move. Commits:
1-line Conventional Commits per numbered item where sensible, no co-author trailer. Do not
merge, push, or build the bundle. Report commit hashes, gate tails, the live transcript, and
anything left out with the reason.
