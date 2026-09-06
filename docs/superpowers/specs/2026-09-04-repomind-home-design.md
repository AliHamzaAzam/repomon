# Repomind Home: a persistent lane and memory for the fleet orchestrator

Status: design approved in principle by the operator on 2026-09-04, for future execution.
Untracked by convention. Owner: coordinating Claude session (PM).

## Decisions (operator, 2026-09-04)

| Decision | Choice |
|---|---|
| Home location | `~/repomind`, its own private git repo (private GitHub remote for sync) |
| Lane model | The Repomind lane replaces the daemon-owned `orchestrator` tmux window; any agent spawned in that lane is a controller with the full fleet catalog |
| Memory store | Markdown files in the home folder, served through basic-memory as a second project beside mnemind; the daemon exports its own records into it |
| Sidebar | A pinned Repomind row at the top of the fleet sidebar; the home repo is hidden from the ordinary repo groups |

## Relationship to the July 2026 "daily driver" plan (decided 2026-09-04: mix)

`docs/superpowers/plans/2026-07-20-repomind-daily-driver.md` (on main) and its phase plans (on
`origin/repomind`) built the substance that is already shipped: repo notes, the orchestration
journal with a cold-start recap, approval-gated playbooks, standing schedules, approval-policy
memory, and the phone loop. Its locked rules stay in force here:

- Memory is served to controllers through `repomon-mcp` unconditionally (every agent kind gets
  it); files and basic-memory are additional layers, not the only path.
- Playbooks written by Repomind are drafts until a human approves them: drafts live in
  `playbooks/drafts/`, approval moves a file into `playbooks/`, and the boot context reads only
  approved ones.
- Live state beats memory: fleet truth comes from `fleet_status`/`read_agent`, never from notes.
- Repomind is a tech lead, not an IC: controllers have the fleet catalog and may write files
  only inside the home repo; code work goes to workers in project lanes.
- Unattended (standing) runs stay more conservative than attended ones.

Split of storage: SQLite stays canonical for machine-written records (journal, schedules,
approval events and rules), exported to markdown one way. Human-authored knowledge is file-first
in the home (repo notes at `fleet/<repo>/notes.md`, playbooks, plans, knowledge); the daemon
reads and writes those files directly and the existing `repo_notes`/`playbook_*` tools operate
on them. The shipped persona asset (`crates/repomon-mcp/assets/repomind.md`) stays authoritative
for tool semantics and safety; `~/repomind/REPOMIND.md` is the operator's overlay (voice,
defaults, house rules) appended at boot. The MCP `since_you_last_looked` recap remains for CLIs
that cannot take a boot file.

## Today

- Repomind is one daemon-owned tmux window named `orchestrator` (`ORCHESTRATOR_WINDOW` in
  `crates/repomon-daemon/src/lib.rs`), started by `orchestrator.start` with cwd = the user's
  home, running an MCP-capable CLI (Claude by default; Codex, Antigravity, OpenCode allowed)
  wired to `repomond mcp`.
- `repomon-mcp` exposes about 25 fleet tools; `REPOMON_MCP_MODE=agent` selects the restricted
  worker catalog, anything else gets the full catalog. The policy layer (autonomy level, action
  cap, send dedupe, two-phase delete confirm, unattended merge refusal) lives there.
- Persistent state is all in the daemon's SQLite: orchestration journal, playbooks, standing
  schedules, approval rules, repo notes. Nothing survives as files; nothing is readable by an
  agent that is not talking to the MCP server.
- The old `repomind` git branch is a stale duplicate: schedules, approvals, playbooks, and the
  journal are on main via other commits. Only `a367f46` (per-connection `orchestrator.watch`)
  may still be worth cherry-picking; then the branch and its worktree go.
- mnemind: `~/mnemind` markdown vault with `profile/`, `projects/`, `knowledge/`, `sessions/`,
  an `AGENTS.md` protocol, served by the basic-memory MCP (`~/.basic-memory/config.json`
  projects `main` and `mnemind`).

## Design

### 1. The home folder (`~/repomind`)

A git repo, created by the daemon on first use if missing (template below), never deleted or
merged by Repomind itself (hard refusal in the policy layer, like `.git` deletion in the editor).

```
~/repomind/
  AGENTS.md            protocol for every agent that runs here (adapted from mnemind's)
  REPOMIND.md          the persona: what Repomind is, autonomy defaults, caps, voice
  profile/             standing facts about the operator's fleet (repos, lanes, agents, quotas)
  plans/
    active/            one file per goal in flight (owner agent, status, next step)
    standing/          the standing orchestrations, mirrored from the daemon schedules
    done/              closed goals, moved here with an outcome line
  playbooks/           mirrored from the daemon store (id in frontmatter, editable both ways)
  fleet/<repo>/        per-repo notes mirrored from repo notes, plus agent-written learnings
  journal/YYYY-MM-DD.md  daily digest exported from the orchestration journal
  knowledge/           cross-cutting facts (tools, conventions, incidents and their fixes)
  sessions/            optional per-session digests written by controller agents on exit
  .repomind/           daemon-owned: boot.md (assembled context), export state, locks
```

Notes follow the mnemind protocol: frontmatter with `title`, `type`, `source` (agent + date),
`permalink`; one concept per note; `[[wiki links]]` including cross-links into mnemind notes by
title. Secrets, tokens, and transient state never go in.

### 2. Lane model and spawning

- Config: `repomind.home = "~/repomind"` (default), `repomind.primary_agent` (default the
  existing `orchestrator_agent`), `repomind.max_controllers` (default 2).
- On daemon start and on `orchestrator.start`: ensure the home repo exists, `repo.add` it if it
  is not registered, ensure one lane on its default branch marked `role = controller` (new
  nullable `lanes.role` column, migration `00NN_lane_role.sql`, next number in sequence).
- `orchestrator.start` spawns the primary agent into that lane's window with
  `REPOMON_MCP_MODE=orchestrator` (full catalog) and the boot context (section 4) instead of a
  separate `orchestrator` window. `orchestrator.*` RPCs stay as thin aliases onto the controller
  lane's primary window for one release, then are deprecated.
- `spawn_agent`/`agent.spawn` into the controller lane produces a controller (full catalog,
  same policy caps per session) up to `max_controllers`; a worker in an ordinary lane cannot
  spawn into the controller lane (policy refusal). Workers keep the restricted catalog.
- Fleet mail: the alias `repomind` resolves to the controller lane's primary window; the lane
  is exempt from thread hop exhaustion when the operator is the other party (already
  configurable via `message_hop_refresh_senders`).
- The Repomind panel (`mod+5`) becomes a view of the controller lane (agent list, boot context,
  active plans, journal tail); Multitasking and Supervision treat controller agents like any
  other, with the supervision policy defaulting to `hold` on destructive classes.

### 2b. Fleet sidebar (decided 2026-09-04)

The controller lane does not appear as a repo group. Instead the fleet sidebar gets one pinned
row above the repo groups, mirroring the TUI's pinned repomind line:

- Content: brain icon, "Repomind", state pill (OFF when no controller is running, otherwise the
  most urgent controller state using the fleet's one-word vocabulary), controller agent count,
  active goal count from `plans/active/`, and the same needs-you pip a repo header carries.
- Behavior: click focuses the controller lane's agents in the terminal bay like any lane;
  right-click offers Start Repomind, Stop, Open panel, Open home in editor; the Repomind panel
  (`mod+5`) remains the detail view (plans, journal tail, boot context, mail). Keyboard: the row
  is the first stop of the sidebar's arrow navigation.
- The `~/repomind` repo is registered like any repo but flagged `role = controller`; the sidebar
  and the fleet filters exclude it from the groups and counts (its agents are counted in the
  pinned row instead), Multitasking and Supervision include its agents normally.
- Filters: "Needs you" and "Running" chips count controller agents too, so the row lights up
  consistently with the rest of the fleet.
- Toolbar: the existing "Repomind" toolbar button (`mod+5`) stays as the panel toggle and gains
  a small state indicator like Repomail's unread badge (signal dot when a controller is running,
  attention color when one needs you, nothing when off). The panel header carries Start, Stop,
  Spawn controller, and Open home in editor; the sidebar row's context menu mirrors them.

### 3. Memory

- basic-memory project `repomind` pointing at `~/repomind` (`basic-memory project add`), so Claude
  controllers get `search_notes`/`read_note`/`write_note` over it with the same tools they use
  for mnemind; the daemon keeps `~/.basic-memory/config.json` in sync (add the project if
  absent, never remove others).
- Non-Claude controllers (Codex, Antigravity, OpenCode) read and write the files directly;
  `AGENTS.md` tells them the layout and the search-before-write rule.
- Daemon export (one way, idempotent, debounced 5 s, and on a `repomind.export` RPC): journal
  rows -> `journal/YYYY-MM-DD.md` (appended sections keyed by row id), schedules ->
  `plans/standing/<slug>.md`, approval rules -> `profile/approvals.md`.
- File-first records (the daemon reads and writes the files, with the SQLite rows retired after a
  one-time migration in R2): repo notes at `fleet/<repo>/notes.md` (the `repo_notes` tools and
  the panel edit the file), playbooks at `playbooks/<slug>.md` with drafts under
  `playbooks/drafts/` (`playbook_save` writes a draft; approval in the CLI or panel moves it;
  `playbook_search` reads approved files only).
- The daemon commits its own exports to the home repo (`chore(repomind): export <what>` as
  author "Repomind <repomind@local>") so history is inspectable; agent-written notes are
  committed by the agents themselves per `AGENTS.md`.
- Trim policy: journal digests older than 90 days roll into `journal/archive/YYYY-MM.md`.

### 4. Boot context

At spawn the daemon assembles `.repomind/boot.md` (bounded, about 12k tokens max):
`REPOMIND.md` + `profile/*` + `plans/active/*` (status lines only) + yesterday's and today's
journal digest + the fleet snapshot (one line per lane). Claude receives it through
`--append-system-prompt-file`; Codex and Antigravity receive a first prompt line pointing at the
file; OpenCode via its instructions file. The file is regenerated on every spawn and on demand
(`repomind.boot` RPC), never hand-edited.

### 5. Safety and policy

- The policy layer in `repomon-mcp` gains: refusal to `delete_lane`/`merge_lane` on the
  controller lane, refusal to spawn controllers from a worker identity, and a per-home write
  budget (files per action) so a runaway agent cannot flood the vault.
- Nothing in `~/repomind` is executed; it is data. `AGENTS.md` forbids storing credentials.
- Remote bridge: `repomind.*` read RPCs allowed (boot, export status), writes local-only.

## Phases (each shippable; sizes S/M/L)

- **R0 (S) Housekeeping.** Cherry-pick `a367f46` if still wanted, delete the `repomind` branch
  and its `.claude/worktrees/feat-repo-notes` worktree. Write `~/repomind` template files by hand
  as the first version of the persona and protocol (no code).
- **R1 (M) Home repo + controller lane.** Config keys, `lanes.role` migration, ensure-home on
  start, `orchestrator.start` spawning into the lane with `REPOMON_MCP_MODE=orchestrator`, RPC
  aliases, MCP catalog selection by role, policy refusals. Acceptance: fresh daemon creates the
  home, `orchestrator.start` lands in the lane, a Codex controller spawned there has the full
  catalog, a worker cannot spawn into it.
- **R2 (M) Export + basic-memory.** Daemon export of journal, playbooks, repo notes, schedules
  into markdown with round-trip for playbooks; basic-memory project registration; the export
  commits. Acceptance: a journal row appears in today's digest within 5 s; editing a playbook
  file updates the store; `search_notes` on project `repomind` finds it.
- **R3 (M) Boot context.** `boot.md` assembly with size bounding and per-backend delivery;
  `repomind.boot` RPC; the panel shows the current boot context. Acceptance: a fresh Claude
  controller can answer "what goals are active" from the boot alone.
- **R4 (M) Panel, sidebar, and Multitasking.** The pinned Repomind sidebar row (section 2b),
  the Repomind panel re-based on the controller lane (agents, plans, journal tail, mail),
  controller agents in Multitasking, supervision defaults for the lane. Design skills
  mandatory; no emoji; no em-dashes.
- **R5 (S) Docs and deprecation.** `docs/desktop.md`, `docs/architecture.md`, `docs/protocol.md`
  updated; `orchestrator.*` marked deprecated in favor of lane RPCs.

Dependencies: R1 -> R2 -> R3 -> R4; R0 and the template files first. Roughly 3 to 4 working
days of agent time with the Antigravity plus Sonnet-fallback pattern used for the editor.

## Risks

- Two memory systems (daemon SQLite and files) drifting: mitigated by one-way export for all but
  playbooks, ids in frontmatter, and an `export` status in the panel.
- A controller with the full catalog in a lane that Multitasking can show and Supervision can
  auto-answer: keep supervision defaults at `hold` for controllers and keep the action cap.
- Boot context growth: hard token bound and a "what was trimmed" line at the end of boot.md.
- Git noise in the home repo from frequent exports: debounce and batch per minute.
