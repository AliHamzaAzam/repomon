# Extensions manager: GUI skill & plugin management

Design approved 2026-07-24. Adds first-class management of Claude Code skills and plugins to Repomon Desktop, global and per-repo, with the daemon as the single authority.

## Problem

Managing what Claude can do today means hand-editing files across two scopes: global (`~/.claude/skills/*/SKILL.md`, `enabledPlugins` in `~/.claude/settings.json`, `~/.claude/plugins/{installed_plugins.json, known_marketplaces.json}`) and per-project (`.claude/skills/`, project settings). There is no view of "what is active where", and repomon adds a twist: agents run in git worktrees, so per-repo config must reach every lane.

Verified facts this design rests on:

- The `claude` CLI exposes the full plugin lifecycle: `plugin install|enable|disable|list|details|prune|new` and `plugin marketplace add|remove|update`. `plugin new` scaffolds into `~/.claude/skills/<name>/`, which auto-loads as `<name>@skills-dir`, so skills and plugins share one enable/disable model.
- Lane creation runs a bare `git worktree add` and never touches `.claude`. Committed `.claude` files reach worktrees via git; gitignored ones (like `settings.local.json`) do not.
- `enabledPlugins` is a flat `{"plugin@marketplace": bool}` map inside `settings.json`, which holds ~15 unrelated keys that must be preserved.

## Decisions (locked with user)

1. **Scope**: full lifecycle. View, toggle, install, remove, update plugins; add/remove/refresh marketplaces; create/edit/delete skills.
2. **Granularity**: per-repo, shared across that repo's lanes. No per-lane overrides in v1.
3. **Architecture**: daemon-mediated. New RPCs on repomond; the GUI stays a thin client; the TUI can adopt the same RPCs later.
4. **Placement**: hybrid. A dedicated Extensions view (sidebar-footer puzzle icon, keyboard `6`) plus right-click quick toggles on each repo row in the fleet sidebar.
5. **Layout**: unified inventory. One searchable list mixing plugins and skills, badged by kind (`plugin`/`skill`) and source (`official`/`user`/`project`), filter chips (All | Plugins | Skills | Marketplaces), scope tabs (Global | one per repo), and a detail drawer on row click.
6. **MVP account**: default `~/.claude` only. The lane model already carries a `home` variant field, so multi-account (`~/.claude-work`) layers onto the same scope machinery later.

## Architecture

All operations flow through new daemon RPCs. The daemon does the work three ways:

- **Read (live view)**: scan config files directly. `~/.claude/skills/*/SKILL.md` frontmatter, repo `.claude/skills/*`, `installed_plugins.json`, `known_marketplaces.json`, and `enabledPlugins` from the relevant settings files. No subprocess, instant, pollable.
- **Toggle / author**: edit `enabledPlugins` in the right settings file; create/delete skill directories.
- **Install / update / marketplace**: shell out to the `claude` CLI so marketplace resolution, caching, and dependency pruning are never reimplemented.

### The repo scope and worktree fan-out

Repo scope treats the repo-root `.claude` as the source of truth. On any repo-scoped mutation, the daemon fans the change out to every lane's worktree `.claude/` (worktree paths come from the existing `git worktree list` cache in `lane.rs`). `lane.create` seeds new worktrees from the repo root. Fan-out is best-effort per lane: each worktree sync is independent, and the response carries a `synced_lanes` / `skipped_lanes` summary instead of failing the whole operation. Committed skills also propagate via git naturally; the sync covers gitignored files and worktrees on older branches.

## RPC surface

Shared scope argument: `{scope: "global"}` or `{scope: "repo", repo_id}`.

Reads:

- `ext.list {scope, repo_id?}` returns one snapshot `{marketplaces[], plugins[], skills[]}`.
  - `plugins[]`: `{id, name, marketplace, version, enabled, enabled_source: global|repo|default, description, provides: {skills, commands, agents, hooks, mcp}, installed}`
  - `skills[]`: `{name, description, source: user|project|plugin, path, plugin?}`
  - `marketplaces[]`: `{name, source: {kind: github|url|local, ref}, plugin_count, last_updated}`
- `plugin.details {id}` shells `claude plugin details` for component inventory and projected token cost.

Mutations, plugins and marketplaces (shell `claude`):

- `plugin.enable` / `plugin.disable {id, scope, repo_id?}`: edits `enabledPlugins`; repo scope fans out.
- `plugin.install {ref, scope, repo_id?}` (`plugin@marketplace`), `plugin.remove {id, scope, repo_id?}`, `plugin.update {id?}`.
- `marketplace.add {source}` / `marketplace.remove {name}` / `marketplace.refresh {name?}`.

Mutations, skills (in-app authoring):

- `skill.create {scope, repo_id?, name, description?}`: `claude plugin new` for global; scaffold `.claude/skills/<name>/SKILL.md` for repo, then fan out.
- `skill.read {path}` / `skill.write {path, content}`: powers the in-app markdown editor.
- `skill.delete {scope, repo_id?, name}` and `skill.reveal {path}` (open in Finder/editor via Tauri opener).

Cross-cutting rules:

1. **Settings writes are surgical and atomic.** Read-modify-write only `enabledPlugins`, preserve every other key, temp-file + rename under a per-file lock.
2. **Install/update run to completion in the daemon** and return `{ok, stdout}` or a structured error `{code, message, data: {stderr, exit_code}}`. On success the daemon emits `event.ext.changed {scope, repo_id?}` so all clients refresh. Progress streaming is a later enhancement.
3. **Fan-out is best-effort per lane** with a synced/skipped summary and retry.

New ts-rs types in `repomon-core` (feature-gated `ts`): `PluginInfo`, `SkillInfo`, `MarketplaceInfo`, `ExtScope`.

## UI

- **Extensions view** (center area; fleet sidebar stays): scope tabs across the top, then a search field with filter chips, then the unified list. Header actions: `+ Install plugin`, `+ New skill`. Marketplaces render behind their chip with add/remove/refresh.
- **Detail drawer** (right, on row click): description, version, provides inventory, token cost (`plugin.details`, lazy), per-scope enable toggles, Update/Remove for plugins, Edit/Delete/Reveal for skills. Skill Edit opens a markdown editor modal backed by `skill.read`/`skill.write`.
- **Repo-row quick toggles**: right-click a repo row shows `Extensions…` (jumps to the view with that repo's scope tab active) plus the enabled-plugin list with instant toggles.

## Behavior and error handling

- **Apply semantics**: running agents never pick up changes; Claude Code reads config at session start. Every mutation surfaces a quiet "applies to new agent sessions" hint. No auto-restart of agents.
- **`claude` CLI missing or old**: detected once at daemon start. Reads, toggles, and skill authoring still work (pure file ops); install/update/marketplace/details controls disable with a "requires claude CLI" hint.
- **CLI op fails**: structured error; GUI shows a toast and the stderr in the drawer.
- **Fan-out partial failure**: synced/skipped summary ("synced 3 lanes, skipped 1: worktree locked") with retry.

## Testing

- Rust units: config scanner against fixture dirs; surgical settings write preserves untouched keys; fan-out against a temp git repo with real worktrees; CLI-missing degradation. Fake the `claude` binary with a PATH shim.
- Frontend: vitest with canned `ext.list` fixtures for list/filter/drawer states, error toasts, quick-toggle menu.
- E2E: extend the isolated harness (it already owns `XDG_CONFIG_HOME`) with a fake `claude` shim; toggle a plugin in the GUI, assert the settings file and a worktree both changed.

## Milestones

- **D1**: daemon read + toggle RPCs (`ext.list`, `plugin.enable/disable`, settings writer, fan-out, `event.ext.changed`).
- **D2**: Extensions view + drawer + repo-row quick toggles (reads and toggles only).
- **D3**: install/update/marketplaces via the CLI, `plugin.details`, degradation path.
- **D4**: skill authoring (create/edit/delete + editor modal), lane-create seeding.

Later: TUI adoption of the same RPCs, multi-account scopes, install progress streaming.
