# Phase 5: Approval Policy Memory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The human's permission verdicts teach the fleet: after 3 consistent approvals of the same command pattern in a repo, repomind proposes an allowlist entry; once confirmed, the daemon auto-approves matching routine permission dialogs (so they never reach the phone) — while force-push/rm -rf/reset --hard always escalate, and denies never generalize.

**Architecture:** A pure `agent::approval` module (pattern extraction from Bash permission dialogs + the always-escalate sniffer), migration 0010 (`approval_events` + `approval_rules`), `approval.*` daemon RPCs (local-only), auto-approve wiring in notify_watch's NeedsYou path (send the default-approve key, journal `auto_approve`, suppress the notification), MCP recording hooks in `approve_agent`/`interrupt_agent` plus a two-phase-confirm `approval_allow` tool and an `approval_rules` read tool.

## Global Constraints

- Branch `feat/approval-policy` stacked on `feat/standing-orchestrations`; PR base = that.
- Only **Bash** permission dialogs learn patterns (title starts with "Bash"); everything else always escalates. Pattern = first two command tokens.
- `is_always_escalate` (server-side, checked before any rule): `--force`/`-f` push, `rm -rf`-family, `git reset --hard`, `git clean -f`, `sudo rm`. Rules never override it.
- Deny verdicts reset the consecutive-approval count and are never generalized into auto-deny.
- Allowlist confirmation is two-phase (mint/redeem token, delete_lane pattern) — repomind cannot self-confirm.
- Threshold: 3 consecutive approvals (const `PROPOSE_AFTER`).
- No em-dashes in commit messages; TDD red-first; fmt/clippy clean per commit.

## Tasks

### 1. core `agent::approval` (pure)
`crates/repomon-core/src/agent/approval.rs` (+ mod in agent/mod.rs):
- `pub fn dialog_command(d: &PendingDialog) -> Option<String>` — title starts with "Bash" → first non-empty body line, trimmed.
- `pub fn command_pattern(cmd: &str) -> String` — first two whitespace tokens joined (one if single).
- `pub fn is_always_escalate(cmd: &str) -> bool` — the list above, case-insensitive, matched on the whole command text.
Tests: cargo test dialog → "cargo test"; single-token; edit dialog → None; `git push --force`, `git push -f origin main`, `rm -rf /`, `sudo rm x`, `git reset --hard HEAD~1`, `git clean -fd` all escalate; `cargo test`, `git push origin main` do not.

### 2. store (migration 0010)
`approval_events(id PK, repo, pattern, verdict, at)`, `approval_rules(repo, pattern, created_at, PRIMARY KEY(repo,pattern))`.
- `record_approval_event(repo, pattern, verdict) -> u32` (consecutive trailing approves for that repo+pattern; a deny resets).
- `add_approval_rule/remove_approval_rule/list_approval_rules() -> Vec<ApprovalRule>` (model: `ApprovalRule {repo, pattern, created_at}`), `has_approval_rule(repo, pattern) -> bool`.
Tests: three approves → 3; deny then approve → 1; rules CRUD + has.

### 3. daemon RPCs + lock-in
- `approval.record {repo, command, verdict}` → `{pattern, approvals, rule_exists, propose}` (propose = approvals >= 3 && !rule && !always_escalate; command with no pattern → `{pattern: null, ...}` no-op).
- `approval.allow {repo, pattern}` / `approval.remove {repo, pattern}` / `approval.list {}`.
- All four remote-blocked. Integration test: record x3 → propose true; allow → list shows; record again → rule_exists; deny resets; always-escalate command never proposes.

### 4. daemon auto-approve (notify_watch)
In the NeedsYou fire path: session's `pending_dialog` → `dialog_command` → if `!is_always_escalate(cmd)` and `has_approval_rule(repo, pattern)` and the session has a managed `tmux_window` → `tmux.send_key_named(window, "Enter")` (the default-approve key, same assumption as approve_agent's choice=None), journal `action=auto_approve` (repo, lane, pattern), drop the prompt cache entry for that window, and skip the notification. Wiring only (pure parts tested in Task 1/2); e2e rides the end acceptance run.

### 5. MCP tools + recording hooks
- `approve_agent`: after a successful approve of a **permission** dialog, best-effort `approval.record` (verdict approve; command from the primary session's pending_dialog). When the response says `propose`, append `proposal` to the result: "3rd consistent approval of '<pattern>' in <repo> — ask the human; on their yes call approval_allow {repo, pattern}, then confirm with the token".
- `interrupt_agent`: when the lane's primary attention was Permission, best-effort record a deny.
- New tools: `approval_allow {repo, pattern, confirm?}` (record_mutation-gated; two-phase confirm via `mint_confirm(0, "approval:<repo>:<pattern>")`; phase 1 returns impact + token, phase 2 stores the rule) and `approval_rules {}` (ungated read → rules list). Catalog 20.
- mcp_stdio test: catalog 20; `approval_rules` empty; `approval_allow` without confirm mints a token (no rule stored); with the token → rule stored, `approval_rules` lists it; bogus token rejected; read-only refuses allow but allows rules read.

### 6. Persona + CLI
- Persona: extend the approvals paragraph: patterns are recorded automatically; on a `proposal` in an approve_agent result, relay to the human and only after their yes run the approval_allow confirm flow; allowlisted patterns are auto-approved by the daemon and never reach anyone; force-push/rm -rf/reset --hard always escalate no matter what. Guard test: PERSONA contains `approval_allow`.
- CLI `repomon approvals list|allow <repo> <pattern>|remove <repo> <pattern>`.

### 7. Full verification + PR
fmt/clippy/test workspace; push; PR base `feat/standing-orchestrations`; notes: Bash-only learning, auto-approve e2e deferred to acceptance run.
