# Brief R5: Repomind docs consolidation and CLI

Spec: `docs/superpowers/specs/2026-09-04-repomind-home-design.md`, phase R5. R1 to R4 are
merged and installed. Each phase updated docs piecemeal; this phase makes them coherent and
adds the CLI surface so the home is usable from a terminal.

Branch: `feat/repomind-docs-cli` in a fresh worktree at `/private/tmp/repomon-feat-repomind-r5`
from current main.

## Scope

1. **CLI** (`crates/repomon-tui/src/cli.rs`): `repomon repomind status` (prints the
   `repomind.status` result as a short table: home, lane, window, controllers, counts, export,
   boot), `repomon repomind boot` (regenerates and prints path, tokens, trimmed), `repomon
   repomind export` (runs an export, prints files and kinds), `repomon repomind open` (prints
   the home path; with `--editor` opens it via `$EDITOR`). `repomon orchestrate` gains a line
   in its help saying Repomind now runs in the controller lane at the home. Tests for the
   table rendering (pure functions) and one CLI integration test against the isolated daemon.
2. **README**: the "repomind" section rewritten for the home and lane model: what the home is,
   the layout in one short tree, how memory flows (SQLite exports, file-first notes and
   playbooks, basic-memory), the boot context, the sidebar row and panel, and the CLI. Keep the
   guardrails paragraph. No emoji, no em-dashes.
3. **docs/architecture.md, docs/protocol.md, docs/desktop.md, docs/messaging.md**: one pass
   for consistency: the `orchestrator.*` deprecation table (alias, replacement, removal
   target "the release after next"), the `repomind.*` RPC table complete (status, boot,
   export), the `repomind` mail alias, the controller catalog rule (`REPOMON_MCP_MODE`), and
   the supervision seed. Remove statements that describe the old `orchestrator` window as
   current.
4. **Man page and completions**: `repomon man` and `repomon completions` include the new
   subcommands (they are generated from clap, verify the snapshot tests if any).
5. **Deprecation warnings**: `orchestrator.*` RPC handlers log one `warn` per process lifetime
   naming the replacement; `docs/protocol.md` says so.

## Rules and gate

Work only in the worktree; never edit the main checkout; never bind a daemon to
`/tmp/repomon-azaleas.sock` or copy the production database; never kill processes by name
pattern; never `git add -A`; tests use tempdir homes and the basic-memory override. No emoji,
no em-dashes in prose (the protocol table's parameterless cells keep the file's existing
convention). Gate: `cargo test -p repomon-tui`, `-p repomon-daemon`; `bun run check` and
`bun run test` in `apps/desktop` only if any frontend file changes. Commits: 1-line
Conventional Commits, no co-author trailer. Do not merge, push, or build the bundle. Report
commit hashes, gate tails, and anything left out with the reason.
