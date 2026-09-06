# Brief R3: Repomind boot context

Spec: `docs/superpowers/specs/2026-09-04-repomind-home-design.md`, sections 3, 4, and phase R3.
R1 and R2 are merged: the home at `[repomind] home`, the controller lane, `repomind.status`
(with export state and counts), exports of journal/schedules/approvals, file-first repo notes
and playbooks, daemon commits, basic-memory registration.

Branch: `feat/repomind-boot` in a fresh worktree at `/private/tmp/repomon-feat-repomind-r3`
from current main.

## Scope

1. **Boot assembly** (`crates/repomon-daemon/src/repomind/boot.rs`): a pure
   `assemble_boot(home, fleet_snapshot, budget) -> BootDocument { markdown, trimmed: Vec<String> }`
   that concatenates, in order: `REPOMIND.md` (operator overlay, whole), `profile/*.md` (bodies
   only, frontmatter stripped), `plans/active/*.md` reduced to one status line each (title,
   status, owner, next step from the body's "Next step" line or the first line), today's and
   yesterday's `journal/YYYY-MM-DD.md` (bodies), and a fleet snapshot (one line per lane: repo,
   lane, branch, agent count, most urgent status). Hard budget of about 12k tokens (use a
   4-chars-per-token estimate); trim from the least important end (journal first, then profile,
   then plans) and end the document with a "Trimmed: ..." line naming what was cut. Written to
   `.repomind/boot.md` (daemon-owned, never hand-edited, ignored by git per R1's `.gitignore`).
   Tests: ordering, frontmatter stripped, active plan reduced to one line, budget enforced with
   the trim note, empty home yields a minimal document.
2. **Delivery per backend** in `orchestrator.start` and controller `agent.spawn` (reuse the
   spawn path from R1): Claude gets `--append-system-prompt-file .repomind/boot.md` (verify the
   flag name against the installed `claude --help`; fall back to `--append-system-prompt` with
   the content if the file form is unavailable); Codex and Antigravity get a first prompt line
   "Read .repomind/boot.md in your working directory before anything else" typed after the
   session is ready (use the existing verified-line injection so it never types over a busy
   composer); OpenCode via its instructions-file mechanism if one exists, else the same typed
   line. The shipped persona asset stays the system prompt for Claude; the boot file is
   appended, not substituted. Tests at the spawn-spec level (the fake agent backend from the
   isolated recipe records its argv and env).
3. **`repomind.boot` RPC** (local-only): regenerates `.repomind/boot.md` on demand and returns
   `{ path, bytes, tokens_estimate, trimmed }`; also regenerated automatically on every spawn.
   `repomind.status` gains `boot: { generated_at, tokens_estimate, trimmed }`. ts-rs types,
   bindings regenerated, `docs/protocol.md`.
4. **Journal archive rollup** (spec section 3 trim policy, left out of R2): on daemon start and
   once a day, roll `journal/YYYY-MM-DD.md` files older than 90 days into
   `journal/archive/YYYY-MM.md` (append, then delete the day file), committed by the export
   commit path. Test with backdated files.
5. **basic-memory isolation** (R2 follow-up): `basic_memory::config_path()` must honor an
   override so an isolated daemon never touches the operator's real `~/.basic-memory/config.json`:
   read `BASIC_MEMORY_HOME` if set (check basic-memory's own documented env var name with
   `basic-memory --help` or its docs and use that), else `[repomind] basic_memory_config` in
   config, else the default. The isolated recipe in `apps/desktop/e2e/isolated.sh` sets it to a
   temp file. Test that the override is respected.
6. **Docs**: `docs/architecture.md` boot section, `docs/desktop.md` one paragraph on what a fresh
   Repomind knows at start.

Out of scope: the sidebar row, panel UI (R4).

## Rules and gate

Work only in the worktree; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`; every test and live run uses a tempdir home and the basic-memory
override from item 5, never the operator's real `~/repomind` or `~/.basic-memory`. No emoji, no
em-dashes. Gate: `cargo test -p repomon-core`, `-p repomon-daemon`, `-p repomon-mcp`,
`-p repomon-tui`; in `apps/desktop` `bun run check`, `bun run test`, `bun run bindings:check`.
Live check with the isolated recipe: seed a tempdir home with an overlay, two active plans, and
a journal day file; `orchestrator.start` with the fake agent; assert the fake agent's recorded
argv or first typed line references the boot file and that `.repomind/boot.md` contains the
plan lines and the fleet snapshot; run `repomind.boot` and confirm the trim note when the budget
is set tiny. Commits: 1-line Conventional Commits per numbered item where sensible, no
co-author trailer. Do not merge, push, or build the bundle. Report commit hashes, gate tails,
the live transcript, and anything left out with the reason.
