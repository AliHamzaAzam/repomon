# Phase 6: Close the Phone Loop (repomon side) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** A `decision`-class repomind escalation is resolvable end-to-end from the phone: the push arrives with context, the app streams the orchestrator pane, and the typed reply rides the already-allowlisted `orchestrator.send_input`.

**Architecture:** Three repomon-side gaps close: (1) repomind's needs-you edge currently fires only a desktop popup — it now also broadcasts `event.notification` and sends APNs when the bridge is on; (2) `orchestrator.watch` was a daemon-global boolean any client could clobber — it becomes per-connection state aggregated across live sessions (the same migration the viewport slots got), making it safe to (3) allowlist `orchestrator.watch` on the remote bridge (read-only stream gate). The iOS in-app UI lives in the repomon-ios repo and is out of scope here; `orchestrator.resize` stays local-only (an unmediated remote resize is the exact regression `agent.fit` exists to prevent).

## Tasks

1. **Per-connection orchestrator watch.** `ConnSession` gains `watches_orchestrator: AtomicBool`; `orchestrator.watch` flips it on the calling session; `Ctx::orchestrator_watched()` aggregates over live sessions; `stream_orchestrator` uses it; the global `Mutex<bool>` field is removed. RED integration test: watch on via conn A → watched; drop conn A → unwatched (session cleanup); conn B independent.
2. **Remote allowlist `orchestrator.watch`.** Move from blocked to allowed in remote.rs with rationale + tests updated.
3. **Orchestrator needs-you push.** Pure `orchestrator_attention_payload(word, headline) -> (title, body, Value)` (unit-tested: decision/permission/end_of_turn titles, dedup id) + wiring in `check_orchestrator_attention`'s edge block: when `cfg.remote.enabled`, broadcast `event.notification` and `push::send_all(CATEGORY_ALERT)` (independent of the desktop-popup TUI gating — the phone should hear even when a TUI is open... no: mirror the lane path, which pushes remotely regardless of `tui_active`; keep desktop gating as is).
4. Full verification + PR (base `feat/approval-policy`). Notes: iOS app UI tracked in repomon-ios; voice memo follow-on out of scope; `orchestrator.resize` deliberately still blocked.
