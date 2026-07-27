# repomind: From Demo to Daily Driver

> **Status:** roadmap + design decisions. Each phase should get its own detailed task-level plan before implementation (one branch/PR per phase). This document locks the *what* and *why*; the per-phase plans lock the *how*.

**Goal:** Make repomind — the fleet orchestrator (`repomon orchestrate` + TUI command-center) — something you reach for daily, by giving it durable memory, procedural learning, standing duties, and a closed phone loop, instead of a smart-but-amnesiac session you must manually start and re-teach every time.

**Architecture:** All state lives in the daemon (SQLite at the existing store + markdown under the config dir), served to the orchestrator through new MCP tools in `repomon-mcp`. The orchestrator prompt (`crates/repomon-mcp/assets/repomind.md`) is updated to *use* these tools; no intelligence moves into Rust. Server-side caps remain the hard safety layer.

**Tech stack:** existing crates (`repomon-core`, `repomon-daemon`, `repomon-mcp`, `repomon-tui`), SQLite + FTS5 (already used for commit search), tmux, existing notification/APNs/WebSocket bridge.

---

## Diagnosis: why repomind underdelivers today

The machinery is genuinely good — lane lifecycle, attention classification (`end_of_turn` / `permission` / `decision`), server-enforced caps, two-phase delete, verify-before-merge. What's missing is everything that would make it *compound*:

1. **Cold-start amnesia.** The prompt literally opens with "At session start you have no history." Every run re-orients from raw `fleet_status`, knows nothing about any repo beyond its name, and repeats discovery the human already paid for. Memory is delegated to an *optional* external MCP (`basic-memory`) that may not be connected.
2. **Session-bound existence.** repomind exists only while `repomon orchestrate` is running. Nothing happens on a schedule; nothing happens when an agent flips to needs-you at 2am. The daemon watches everything and tells a human, never repomind.
3. **No procedural learning.** The fifth time you run a "fix CI across three repos" goal, repomind plans it from zero, with the same worker-prompt mistakes the human already corrected four times.
4. **No orchestration history.** The daemon indexes *commits* for search, but "what did repomind do last Tuesday and why" lives only in dead tmux scrollback.
5. **Stateless approval judgment.** Every `permission` dialog is re-judged from scratch. The human's past verdicts teach it nothing.
6. **Half-open phone loop.** Pushes and approvals reach the phone, but steering repomind conversationally from the phone isn't first-class — the useful reply to "repomind needs you" is a decision, and typing it should be as easy as tapping the notification.

Fix these six and repomind stops being a demo of orchestration and becomes the thing that runs your fleet while you're at dinner.

---

## Design decisions (locked)

- **Daemon-owned memory, not optional MCP.** Per-repo knowledge and orchestration history are served by `repomon-mcp` itself so every repomind session has them unconditionally. External memory (mnemind) stays supported as an *additional* layer for cross-tool knowledge, per the existing prompt section.
- **Approval-gated procedural learning.** Auto-drafted playbooks are drafts until the human approves them. Self-written instructions feeding back into the orchestrator's own prompt without review is a self-poisoning prompt-injection vector.
- **Live state always beats memory.** The existing prompt rule stays: fleet truth comes from `fleet_status`/`read_agent`, never from notes.
- **repomind stays a tech lead, not an IC.** No phase gives it code-writing tools. Server-side caps (action cap, max-agents, dedupe, two-phase delete) remain enforced in Rust, never relaxed by anything it "learns."
- **Bounded standing runs.** Scheduled/triggered orchestrations get their own (lower) action caps and time limits; an unattended orchestrator must be *more* conservative than an attended one, not less.

---

## Phase 1: Per-repo fleet memory

**The single highest-leverage fix.** A `repo_notes` store the daemon owns: conventions, build/test commands, merge preferences, known gotchas, "always tell workers X."

- New MCP tools: `repo_notes(repo)` → markdown; `repo_notes_write(repo, content)` (full replace, size-capped ~8 KB, audit-logged). Storage: one markdown file per repo under the daemon's app-support dir (human-editable too), path-validated against the repo registry.
- `create_lane` and `spawn_agent` responses embed the repo's notes so repomind folds them into worker prompts without an extra round-trip.
- Prompt update (`repomind.md`): Orient step reads notes for touched repos; after merges or corrected mistakes, write the durable lesson back (edit, don't append-forever).
- TUI: notes visible/editable from the repo view (nice-to-have, can trail).

**Acceptance:** spawn a worker into a repo with notes saying "use `pnpm test`, never `npm test`" → the worker's task prompt contains it, with no human reminder.

## Phase 2: Orchestration journal + cold-start recap

- Daemon logs every orchestrator-initiated action (spawn/send/approve/interrupt/merge/delete + parameters digest + outcome) to a `orchestration_log` table; FTS5 over it plus lane/goal summaries.
- New MCP tool: `fleet_history({query} | {since_last_session: true})` → compact digest.
- `fleet_status` gains a `since_you_last_looked` block (merged lanes, ended agents, escalations) so the first call of a session both orients and recaps.
- Prompt update: delete "you have no history"; Orient becomes `fleet_status` → explain unexplained states via `fleet_history`/`read_agent`.

**Acceptance:** "what happened with the auth refactor last week?" answered from the journal; a fresh `repomon orchestrate` opens with a correct recap instead of re-discovery.

## Phase 3: Playbooks (procedural learning)

- After a goal completes (lanes created → merged/closed), repomind drafts a playbook: goal pattern, per-repo steps, worker prompts that worked, verification steps, failure modes hit. Saved via new `playbook_save` tool as `draft`; `playbook_search(query)` returns approved playbooks only.
- Human approval via TUI command-center or `repomon playbooks approve <name>` CLI. Drafts expire unreviewed after 30 days.
- Prompt update: before planning multi-lane work, `playbook_search`; follow an approved playbook when one matches, noting deviations back into the draft cycle.

**Acceptance:** the second "release all changed repos" run follows the approved playbook from the first, visibly skipping the trial-and-error.

## Phase 4: Standing orchestrations (schedules + triggers)

- Daemon-owned schedules: `repomon orchestrate --schedule "weekdays 09:00" --max-actions 20 "morning fleet briefing"` — spawns a bounded, headless orchestrator run; result delivered through the existing notification path (desktop + APNs), full transcript in the journal.
- Event trigger: when an agent flips to needs-you and no UI is attached for N minutes, run a bounded **triage orchestration**: `read_agent`, classify, then either answer a routine `permission` itself (within policy, Phase 5) or escalate — with the push notification now carrying *context and a recommendation*, not just "agent needs you."
- Hard bounds: separate lower action cap, wall-clock limit, never `merge_lane`/`delete_lane` in unattended mode (report-and-recommend only).

**Acceptance:** at 9am your phone shows "3 lanes merged-ready, 1 decision needed on pos-saas (recommend option B), 2 idle lanes >48h" without you having started anything.

## Phase 5: Approval policy memory

- Record every escalated `permission` verdict as (repo, command pattern, approve/deny). After 3 consistent human approvals of the same pattern in a repo, repomind proposes an allowlist entry; on human confirmation it's stored per-repo and consulted before future escalations (attended and unattended runs both).
- Hardcoded server-side always-escalate patterns regardless of policy: force-push, `rm -rf`, `reset --hard`, anything matching the destructive-command sniffer. Deny verdicts are never generalized into auto-deny — they just keep escalating.

**Acceptance:** the fourth `cargo test` permission in the same repo never reaches your phone; a `git push --force` always does.

## Phase 6: Close the phone loop

- Make conversational steering of repomind first-class on iOS over the existing WebSocket bridge: streamed orchestrator pane + mediated `send_input` (the TUI's `i` flow, exposed remotely). Reply to a "repomind needs you" push by typing the decision in-app.
- Voice memo → local transcription → `send_input` is a natural follow-on once the text path works.

**Acceptance:** a `decision`-class escalation is resolved end-to-end from the phone: push → read context → type answer → repomind relays it to the worker — laptop lid closed.

---

## Sequencing and scope

1 → 2 are the foundation (memory + history) and each is small; 3 depends on 2's journal; 4 depends on 2 (recap for briefings) and is where the daily-driver payoff lands; 5 slots into 4's unattended mode; 6 is independent of 3–5 and can run in parallel with any of them. Every phase is independently shippable; none changes the lane lifecycle, the caps, or the worker-agent layer.
