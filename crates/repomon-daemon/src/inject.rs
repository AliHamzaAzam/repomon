//! Single verified-injection module for supervision actions.
//!
//! Spec constraint: ALL supervision pane interaction goes through ONE module with pre-send
//! state re-verification; no supervision code may call send-keys anywhere else; and NO supervision
//! action may ever be unlogged — every attempt, including skips and failures, writes exactly one
//! `supervision_log` row and broadcasts `event.supervision.acted`.

use std::time::{Duration, Instant};

use chrono::Utc;
use repomon_core::agent::backend::CaptureOpts;
use repomon_core::agent::detect_usage_limit;
use repomon_core::agent::prompt::detect_dialog;
use repomon_core::model::{LaneId, SupervisionEntry};

use crate::{Ctx, pubsub};

/// Anti-thrashing latch cooldown: suppress re-sending the same expectation fingerprint to the same window.
pub const LATCH_COOLDOWN: Duration = Duration::from_secs(90);

/// Hard ceiling on pane capture time before abandoning injection.
pub const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expectation {
    DialogSummary(String), // a dialog must still be present and summary() must equal this
    IdleNoDialog,          // NO dialog AND NO usage-limit menu may be on screen
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Keys(Vec<String>), // sent one by one via backend.send_key_named
    Line(String),      // sent via backend.send_text_named (text + Enter)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    StateChanged,
    DialogPresent,
    UsageLimitMenu,
    CaptureTimeout,
    CaptureFailed,
    LatchHeld,
    WindowGone,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StateChanged => "state_changed",
            Self::DialogPresent => "dialog_present",
            Self::UsageLimitMenu => "usage_limit_menu",
            Self::CaptureTimeout => "capture_timeout",
            Self::CaptureFailed => "capture_failed",
            Self::LatchHeld => "latch_held",
            Self::WindowGone => "window_gone",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuditSeed {
    pub lane_id: LaneId,
    pub window: String,
    pub session_id: Option<String>,
    pub agent_kind: Option<String>,
    pub trigger: String, // "dialog" | "mail" | "stall" | "manual_nudge" | "legacy_rule"
    pub dialog_class: Option<repomon_core::agent::supervision::DialogClass>,
    pub repo_scoped: Option<bool>,
    pub decision: String, // "approve" | "deny" | "nudge" | ...
    pub policy_source: Option<repomon_core::agent::supervision::PolicySource>,
    pub reason: Option<String>,
    pub subject: Option<String>,
    pub pane_excerpt: Option<String>, // verified_send overwrites this with the fresh capture excerpt
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Sent { keys: Vec<String>, entry_id: i64 },
    Skipped { reason: SkipReason, entry_id: i64 },
    Failed { error: String, entry_id: i64 },
}

pub async fn verified_send(
    ctx: &Ctx,
    expect: Expectation,
    payload: Payload,
    seed: AuditSeed,
) -> SendOutcome {
    verified_send_with_timeout(ctx, expect, payload, seed, CAPTURE_TIMEOUT).await
}

pub async fn record_hold(ctx: &Ctx, seed: AuditSeed) -> i64 {
    let entry = SupervisionEntry {
        id: 0,
        at: Utc::now(),
        lane_id: seed.lane_id,
        window: seed.window,
        session_id: seed.session_id,
        agent_kind: seed.agent_kind,
        trigger: seed.trigger,
        dialog_class: seed.dialog_class,
        repo_scoped: seed.repo_scoped,
        decision: "hold".to_string(),
        policy_source: seed.policy_source,
        keys: None,
        outcome: "held".to_string(),
        reason: seed.reason,
        subject: seed.subject,
        pane_excerpt: seed.pane_excerpt,
    };
    let entry_id = ctx
        .store
        .append_supervision(entry.clone())
        .await
        .unwrap_or(0);
    let mut logged_entry = entry;
    logged_entry.id = entry_id;
    ctx.broadcast(
        pubsub::SUPERVISION_ACTED,
        serde_json::to_value(&logged_entry).unwrap_or_default(),
    );
    entry_id
}

pub async fn verified_send_with_timeout(
    ctx: &Ctx,
    expect: Expectation,
    payload: Payload,
    seed: AuditSeed,
    capture_timeout: Duration,
) -> SendOutcome {
    // 1. Latch check
    let fingerprint = expectation_fingerprint(&expect, &payload);
    {
        let latch = ctx.inject_latch.lock().await;
        if let Some((fp, when)) = latch.get(&seed.window) {
            if fp == &fingerprint && when.elapsed() < LATCH_COOLDOWN {
                return finish(
                    ctx,
                    seed,
                    None,
                    InternalOutcome::Skipped(SkipReason::LatchHeld, None),
                )
                .await;
            }
        }
    }

    // 2. Fresh capture
    let window = seed.window.clone();
    let backend = ctx.backend.clone();
    let capture_result = tokio::time::timeout(
        capture_timeout,
        tokio::task::spawn_blocking(move || backend.capture_named(&window, CaptureOpts::last(45))),
    )
    .await;

    let pane_text = match capture_result {
        Err(_elapsed) => {
            return finish(
                ctx,
                seed,
                None,
                InternalOutcome::Skipped(
                    SkipReason::CaptureTimeout,
                    Some("capture timed out".to_string()),
                ),
            )
            .await;
        }
        Ok(Err(join_err)) => {
            return finish(
                ctx,
                seed,
                None,
                InternalOutcome::Skipped(SkipReason::CaptureFailed, Some(join_err.to_string())),
            )
            .await;
        }
        Ok(Ok(Err(backend_err))) => {
            return finish(
                ctx,
                seed,
                None,
                InternalOutcome::Skipped(SkipReason::CaptureFailed, Some(backend_err.to_string())),
            )
            .await;
        }
        Ok(Ok(Ok(text))) => text,
    };

    let excerpt = Some(tail_chars(&pane_text, 800).to_string());

    // 3. Detect dialog and usage limit
    let dialog_opt = detect_dialog(&pane_text);
    let limit_opt = detect_usage_limit(&pane_text);

    // 4. Verify expectation
    match &expect {
        Expectation::DialogSummary(expected_summary) => match dialog_opt {
            Some(ref d) if &d.summary() == expected_summary => {
                // Expectation met
            }
            Some(ref d) => {
                return finish(
                    ctx,
                    seed,
                    excerpt,
                    InternalOutcome::Skipped(
                        SkipReason::StateChanged,
                        Some(format!(
                            "dialog changed: expected '{}', found '{}'",
                            expected_summary,
                            d.summary()
                        )),
                    ),
                )
                .await;
            }
            None => {
                return finish(
                    ctx,
                    seed,
                    excerpt,
                    InternalOutcome::Skipped(
                        SkipReason::StateChanged,
                        Some("dialog no longer present".to_string()),
                    ),
                )
                .await;
            }
        },
        Expectation::IdleNoDialog => {
            if let Some(ref d) = dialog_opt {
                return finish(
                    ctx,
                    seed,
                    excerpt,
                    InternalOutcome::Skipped(
                        SkipReason::DialogPresent,
                        Some(format!("dialog present: {}", d.summary())),
                    ),
                )
                .await;
            }
            if limit_opt.is_some() {
                return finish(
                    ctx,
                    seed,
                    excerpt,
                    InternalOutcome::Skipped(
                        SkipReason::UsageLimitMenu,
                        Some("usage limit menu present".to_string()),
                    ),
                )
                .await;
            }
        }
    }

    // 5. Send
    let recorded_keys = match &payload {
        Payload::Keys(keys) => {
            let send_keys = keys.clone();
            let win = seed.window.clone();
            let backend = ctx.backend.clone();
            let send_res = tokio::task::spawn_blocking(move || {
                for k in &send_keys {
                    backend.send_key_named(&win, k)?;
                }
                Ok::<(), repomon_core::Error>(())
            })
            .await;
            match send_res {
                Ok(Ok(())) => keys.clone(),
                Ok(Err(e)) => {
                    return finish(ctx, seed, excerpt, InternalOutcome::Failed(e.to_string()))
                        .await;
                }
                Err(e) => {
                    return finish(ctx, seed, excerpt, InternalOutcome::Failed(e.to_string()))
                        .await;
                }
            }
        }
        Payload::Line(line) => {
            let send_line = line.clone();
            let win = seed.window.clone();
            let backend = ctx.backend.clone();
            let send_res =
                tokio::task::spawn_blocking(move || backend.send_text_named(&win, &send_line))
                    .await;
            match send_res {
                Ok(Ok(())) => {
                    let lit = format!("<literal:{}>", truncate_chars(line, 120));
                    vec![lit, "Enter".to_string()]
                }
                Ok(Err(e)) => {
                    return finish(ctx, seed, excerpt, InternalOutcome::Failed(e.to_string()))
                        .await;
                }
                Err(e) => {
                    return finish(ctx, seed, excerpt, InternalOutcome::Failed(e.to_string()))
                        .await;
                }
            }
        }
    };

    // 6. Mark input and invalidate overlay
    crate::rpc::mark_input(ctx, seed.lane_id, &seed.window).await;
    ctx.invalidate_overlay().await;

    // 7. Latch stamp
    ctx.inject_latch
        .lock()
        .await
        .insert(seed.window.clone(), (fingerprint, Instant::now()));

    // 8-9. Audit and broadcast
    finish(ctx, seed, excerpt, InternalOutcome::Sent(recorded_keys)).await
}

fn expectation_fingerprint(expect: &Expectation, payload: &Payload) -> String {
    match expect {
        Expectation::DialogSummary(s) => s.clone(),
        Expectation::IdleNoDialog => match payload {
            Payload::Keys(keys) => keys.join(" "),
            Payload::Line(line) => line.clone(),
        },
    }
}

fn tail_chars(s: &str, max_chars: usize) -> &str {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s
    } else {
        let skip = char_count - max_chars;
        match s.char_indices().nth(skip) {
            Some((idx, _)) => &s[idx..],
            None => s,
        }
    }
}

fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

enum InternalOutcome {
    Sent(Vec<String>),
    Skipped(SkipReason, Option<String>),
    Failed(String),
}

async fn finish(
    ctx: &Ctx,
    seed: AuditSeed,
    excerpt: Option<String>,
    outcome: InternalOutcome,
) -> SendOutcome {
    let (outcome_str, keys, reason, result_outcome) = match outcome {
        InternalOutcome::Sent(sent_keys) => (
            "sent".to_string(),
            Some(sent_keys.clone()),
            seed.reason,
            SendOutcome::Sent {
                keys: sent_keys,
                entry_id: 0,
            },
        ),
        InternalOutcome::Skipped(skip_reason, skip_detail) => {
            let reason_str = skip_detail.or_else(|| Some(skip_reason.as_str().to_string()));
            (
                "skipped".to_string(),
                None,
                reason_str,
                SendOutcome::Skipped {
                    reason: skip_reason,
                    entry_id: 0,
                },
            )
        }
        InternalOutcome::Failed(err_msg) => (
            "failed".to_string(),
            None,
            Some(err_msg.clone()),
            SendOutcome::Failed {
                error: err_msg,
                entry_id: 0,
            },
        ),
    };

    let mut entry = SupervisionEntry {
        id: 0,
        at: Utc::now(),
        lane_id: seed.lane_id,
        window: seed.window,
        session_id: seed.session_id,
        agent_kind: seed.agent_kind,
        trigger: seed.trigger,
        dialog_class: seed.dialog_class,
        repo_scoped: seed.repo_scoped,
        decision: seed.decision,
        policy_source: seed.policy_source,
        keys,
        outcome: outcome_str,
        reason,
        subject: seed.subject,
        pane_excerpt: excerpt.or(seed.pane_excerpt),
    };

    let entry_id = ctx
        .store
        .append_supervision(entry.clone())
        .await
        .unwrap_or(0);
    entry.id = entry_id;
    ctx.broadcast(
        pubsub::SUPERVISION_ACTED,
        serde_json::to_value(&entry).unwrap_or_default(),
    );

    match result_outcome {
        SendOutcome::Sent { keys, .. } => SendOutcome::Sent { keys, entry_id },
        SendOutcome::Skipped { reason, .. } => SendOutcome::Skipped { reason, entry_id },
        SendOutcome::Failed { error, .. } => SendOutcome::Failed { error, entry_id },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use repomon_core::agent::backend::{
        AttachCommand, OwnerState, SessionBackend, SpawnSpec, WindowActivity,
    };
    use repomon_core::agent::supervision::{DialogClass, PolicySource};
    use repomon_core::{Config, Store, config};
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;

    struct ScriptedBackend {
        captures: StdMutex<Vec<String>>,
        last_capture: StdMutex<Option<String>>,
        sent_keys: StdMutex<Vec<(String, String)>>,
        sent_text: StdMutex<Vec<(String, String)>>,
        capture_delay: Option<Duration>,
        capture_error: Option<String>,
        send_error: Option<String>,
    }

    impl ScriptedBackend {
        fn new(captures: Vec<String>) -> Self {
            Self {
                captures: StdMutex::new(captures),
                last_capture: StdMutex::new(None),
                sent_keys: StdMutex::new(Vec::new()),
                sent_text: StdMutex::new(Vec::new()),
                capture_delay: None,
                capture_error: None,
                send_error: None,
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.capture_delay = Some(delay);
            self
        }

        fn with_send_error(mut self, err: &str) -> Self {
            self.send_error = Some(err.to_string());
            self
        }
    }

    impl SessionBackend for ScriptedBackend {
        fn available(&self) -> bool {
            true
        }
        fn label(&self) -> String {
            "scripted".to_string()
        }
        fn session_exists(&self) -> bool {
            true
        }
        fn claim_or_verify_owner(&self, _me: &str) -> OwnerState {
            OwnerState::Owned
        }
        fn list_windows(&self) -> repomon_core::Result<Vec<String>> {
            Ok(vec![])
        }
        fn list_windows_with_activity(&self) -> repomon_core::Result<Vec<WindowActivity>> {
            Ok(vec![])
        }
        fn spawn(&self, _lane: LaneId, _spec: &SpawnSpec) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn spawn_named(&self, _window: &str, _spec: &SpawnSpec) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn open_named(
            &self,
            _window: &str,
            _cwd: &std::path::Path,
        ) -> repomon_core::Result<String> {
            Ok("target".into())
        }
        fn capture_named(&self, _window: &str, _opts: CaptureOpts) -> repomon_core::Result<String> {
            if let Some(delay) = self.capture_delay {
                std::thread::sleep(delay);
            }
            if let Some(ref err) = self.capture_error {
                return Err(repomon_core::Error::Agent(err.clone()));
            }
            let mut list = self.captures.lock().unwrap();
            let cap = if !list.is_empty() {
                let next = list.remove(0);
                *self.last_capture.lock().unwrap() = Some(next.clone());
                next
            } else {
                self.last_capture
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_default()
            };
            Ok(cap)
        }
        fn cursor_named(&self, _window: &str) -> Option<repomon_core::agent::Cursor> {
            None
        }
        fn size_named(&self, _window: &str) -> Option<(u16, u16)> {
            Some((80, 24))
        }
        fn resize_named(&self, _window: &str, _cols: u16, _rows: u16) -> repomon_core::Result<()> {
            Ok(())
        }
        fn follow_client_named(&self, _window: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn alternate_on_named(&self, _window: &str) -> bool {
            false
        }
        fn scroll_wheel_named(
            &self,
            _window: &str,
            _event: repomon_core::agent::backend::ScrollEvent,
        ) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_literal_named(&self, _window: &str, _text: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn send_text_named(&self, window: &str, text: &str) -> repomon_core::Result<()> {
            if let Some(ref err) = self.send_error {
                return Err(repomon_core::Error::Agent(err.clone()));
            }
            self.sent_text
                .lock()
                .unwrap()
                .push((window.to_string(), text.to_string()));
            Ok(())
        }
        fn send_key_named(&self, window: &str, key: &str) -> repomon_core::Result<()> {
            if let Some(ref err) = self.send_error {
                return Err(repomon_core::Error::Agent(err.clone()));
            }
            self.sent_keys
                .lock()
                .unwrap()
                .push((window.to_string(), key.to_string()));
            Ok(())
        }
        fn kill_named(&self, _window: &str) -> repomon_core::Result<()> {
            Ok(())
        }
        fn configure(&self) {}
        fn target_named(&self, window: &str) -> String {
            window.to_string()
        }
        fn exact_target_named(&self, window: &str) -> String {
            window.to_string()
        }
        fn attach_command(&self, target: &str) -> AttachCommand {
            AttachCommand {
                program: "tmux".into(),
                args: vec!["attach".into(), "-t".into(), target.into()],
            }
        }
        fn open_byte_stream(
            &self,
            _window: &str,
        ) -> repomon_core::Result<repomon_core::agent::backend::ByteStream> {
            let (_tx, rx) = tokio::sync::mpsc::unbounded_channel();
            Ok(repomon_core::agent::backend::ByteStream { rx })
        }
        fn close_byte_stream(&self, _window: &str) -> repomon_core::Result<()> {
            Ok(())
        }
    }

    fn test_seed(window: &str) -> AuditSeed {
        AuditSeed {
            lane_id: 1,
            window: window.to_string(),
            session_id: Some("sess-1".to_string()),
            agent_kind: Some("claude-code".to_string()),
            trigger: "dialog".to_string(),
            dialog_class: Some(DialogClass::CommandExec),
            repo_scoped: Some(true),
            decision: "approve".to_string(),
            policy_source: Some(PolicySource::ApprovalRule),
            reason: Some("rule matched".to_string()),
            subject: Some("cargo install".to_string()),
            pane_excerpt: None,
        }
    }

    fn make_ctx(backend: Arc<dyn SessionBackend>) -> Arc<Ctx> {
        let store = Store::open_in_memory().unwrap();
        Ctx::new_with_backend(
            store,
            Config::default(),
            None,
            config::config_path(),
            config::data_dir().join("repo-notes"),
            backend,
        )
    }

    const DIALOG_A: &str = "● Running cargo test…\n\
        ╭──────────────────────────────────────────────╮\n\
        │ Bash command                                 │\n\
        │                                              │\n\
        │   cargo install --path crates/repomon-tui    │\n\
        │   Install the repomon TUI                    │\n\
        │                                              │\n\
        │ Do you want to proceed?                      │\n\
        │ ❯ 1. Yes                                     │\n\
        │   2. Yes, and don't ask again for cargo      │\n\
        │   3. No, and tell Claude what to do          │\n\
        ╰──────────────────────────────────────────────╯";

    const DIALOG_B: &str = "  Field 1/1\n\
        Allow the repomon MCP server to run tool \"fleet_status\"?\n\
        › 1. Allow                   Run the tool and continue.\n\
          2. Allow for this session  Run the tool and remember this choice for this session.\n\
          3. Always allow            Run the tool and remember this choice for future tool calls.\n\
          4. Cancel                  Cancel this tool call\n\
        enter to submit | esc to cancel";

    const USAGE_LIMIT_MENU: &str = "What do you want to do?\n\
        ❯ 1. Stop and wait for limit to reset\n\
          2. Upgrade your plan\n\
          3. Upgrade to Team plan\n\
        Enter to confirm · Esc to cancel";

    const IDLE_PANE: &str = "azaleas@macbook repomon % cargo check\n    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s\nazaleas@macbook repomon % ";

    #[tokio::test]
    async fn dialog_changed_between_snapshot_and_send_skips() {
        let backend = Arc::new(ScriptedBackend::new(vec![DIALOG_B.to_string()]));
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::DialogSummary("Bash command — Do you want to proceed?".into());
        let payload = Payload::Keys(vec!["1".into(), "Enter".into()]);
        let seed = test_seed("lane-1");

        let outcome = verified_send(&ctx, expect, payload, seed).await;
        match outcome {
            SendOutcome::Skipped { reason, entry_id } => {
                assert_eq!(reason, SkipReason::StateChanged);
                assert!(entry_id > 0);
            }
            other => panic!("expected Skipped(StateChanged), got {:?}", other),
        }

        assert!(backend.sent_keys.lock().unwrap().is_empty());
        assert!(backend.sent_text.lock().unwrap().is_empty());

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "skipped");
        assert_eq!(log[0].keys, None);
    }

    #[tokio::test]
    async fn idle_expectation_with_dialog_on_screen_skips() {
        let backend = Arc::new(ScriptedBackend::new(vec![DIALOG_A.to_string()]));
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::IdleNoDialog;
        let payload = Payload::Line("cargo test".into());
        let seed = test_seed("lane-1");

        let outcome = verified_send(&ctx, expect, payload, seed).await;
        match outcome {
            SendOutcome::Skipped { reason, entry_id } => {
                assert_eq!(reason, SkipReason::DialogPresent);
                assert!(entry_id > 0);
            }
            other => panic!("expected Skipped(DialogPresent), got {:?}", other),
        }

        assert!(backend.sent_keys.lock().unwrap().is_empty());
        assert!(backend.sent_text.lock().unwrap().is_empty());

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "skipped");
    }

    #[tokio::test]
    async fn idle_expectation_with_usage_limit_menu_skips() {
        let backend = Arc::new(ScriptedBackend::new(vec![USAGE_LIMIT_MENU.to_string()]));
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::IdleNoDialog;
        let payload = Payload::Line("cargo test".into());
        let seed = test_seed("lane-1");

        let outcome = verified_send(&ctx, expect, payload, seed).await;
        match outcome {
            SendOutcome::Skipped { reason, entry_id } => {
                assert_eq!(reason, SkipReason::UsageLimitMenu);
                assert!(entry_id > 0);
            }
            other => panic!("expected Skipped(UsageLimitMenu), got {:?}", other),
        }

        assert!(backend.sent_keys.lock().unwrap().is_empty());
        assert!(backend.sent_text.lock().unwrap().is_empty());

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "skipped");
    }

    #[tokio::test]
    async fn happy_path_sends_exact_keys_and_journals() {
        let backend = Arc::new(ScriptedBackend::new(vec![DIALOG_A.to_string()]));
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::DialogSummary("Bash command — Do you want to proceed?".into());
        let payload = Payload::Keys(vec!["1".into(), "Enter".into()]);
        let seed = test_seed("lane-1");

        let outcome = verified_send(&ctx, expect, payload, seed).await;
        match outcome {
            SendOutcome::Sent { keys, entry_id } => {
                assert_eq!(keys, vec!["1", "Enter"]);
                assert!(entry_id > 0);
            }
            other => panic!("expected Sent, got {:?}", other),
        }

        let sent = backend.sent_keys.lock().unwrap().clone();
        assert_eq!(
            sent,
            vec![
                ("lane-1".to_string(), "1".to_string()),
                ("lane-1".to_string(), "Enter".to_string())
            ]
        );

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "sent");
        assert_eq!(log[0].keys, Some(vec!["1".into(), "Enter".into()]));
    }

    #[tokio::test]
    async fn latch_blocks_second_send_within_cooldown() {
        let backend = Arc::new(ScriptedBackend::new(vec![
            DIALOG_A.to_string(),
            DIALOG_A.to_string(),
        ]));
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::DialogSummary("Bash command — Do you want to proceed?".into());
        let payload = Payload::Keys(vec!["1".into(), "Enter".into()]);
        let seed = test_seed("lane-1");

        let first = verified_send(&ctx, expect.clone(), payload.clone(), seed.clone()).await;
        assert!(matches!(first, SendOutcome::Sent { .. }));

        let second = verified_send(&ctx, expect, payload, seed).await;
        match second {
            SendOutcome::Skipped { reason, entry_id } => {
                assert_eq!(reason, SkipReason::LatchHeld);
                assert!(entry_id > 0);
            }
            other => panic!("expected Skipped(LatchHeld), got {:?}", other),
        }

        // Only the first send's keys reached the backend
        assert_eq!(backend.sent_keys.lock().unwrap().len(), 2);

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].outcome, "skipped"); // newest first
        assert_eq!(log[1].outcome, "sent");
    }

    #[tokio::test]
    async fn capture_timeout_skips_and_journals() {
        let backend = Arc::new(
            ScriptedBackend::new(vec![DIALOG_A.to_string()]).with_delay(Duration::from_millis(150)),
        );
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::DialogSummary("Bash command — Do you want to proceed?".into());
        let payload = Payload::Keys(vec!["1".into(), "Enter".into()]);
        let seed = test_seed("lane-1");

        let outcome =
            verified_send_with_timeout(&ctx, expect, payload, seed, Duration::from_millis(20))
                .await;

        match outcome {
            SendOutcome::Skipped { reason, entry_id } => {
                assert_eq!(reason, SkipReason::CaptureTimeout);
                assert!(entry_id > 0);
            }
            other => panic!("expected Skipped(CaptureTimeout), got {:?}", other),
        }

        assert!(backend.sent_keys.lock().unwrap().is_empty());
        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "skipped");
    }

    #[tokio::test]
    async fn audit_completeness_property() {
        let backend = Arc::new(ScriptedBackend::new(vec![
            DIALOG_A.to_string(),
            IDLE_PANE.to_string(),
            DIALOG_B.to_string(),
            DIALOG_A.to_string(),
            USAGE_LIMIT_MENU.to_string(),
            IDLE_PANE.to_string(),
        ]));
        let ctx = make_ctx(backend.clone());

        // 1. Sent (keys)
        let s1 = verified_send(
            &ctx,
            Expectation::DialogSummary("Bash command — Do you want to proceed?".into()),
            Payload::Keys(vec!["1".into()]),
            test_seed("lane-1"),
        )
        .await;
        assert!(matches!(s1, SendOutcome::Sent { .. }));

        // 2. Sent (line)
        let s2 = verified_send(
            &ctx,
            Expectation::IdleNoDialog,
            Payload::Line("cargo test".into()),
            test_seed("lane-2"),
        )
        .await;
        assert!(matches!(s2, SendOutcome::Sent { .. }));

        // 3. Skipped (StateChanged)
        let s3 = verified_send(
            &ctx,
            Expectation::DialogSummary("Bash command — Do you want to proceed?".into()),
            Payload::Keys(vec!["1".into()]),
            test_seed("lane-3"),
        )
        .await;
        assert!(matches!(
            s3,
            SendOutcome::Skipped {
                reason: SkipReason::StateChanged,
                ..
            }
        ));

        // 4. Skipped (DialogPresent)
        let s4 = verified_send(
            &ctx,
            Expectation::IdleNoDialog,
            Payload::Line("cargo check".into()),
            test_seed("lane-4"),
        )
        .await;
        assert!(matches!(
            s4,
            SendOutcome::Skipped {
                reason: SkipReason::DialogPresent,
                ..
            }
        ));

        // 5. Skipped (UsageLimitMenu)
        let s5 = verified_send(
            &ctx,
            Expectation::IdleNoDialog,
            Payload::Line("cargo check".into()),
            test_seed("lane-5"),
        )
        .await;
        assert!(matches!(
            s5,
            SendOutcome::Skipped {
                reason: SkipReason::UsageLimitMenu,
                ..
            }
        ));

        // 6. Skipped (LatchHeld)
        let s6 = verified_send(
            &ctx,
            Expectation::IdleNoDialog,
            Payload::Line("cargo test".into()),
            test_seed("lane-2"),
        )
        .await;
        assert!(matches!(
            s6,
            SendOutcome::Skipped {
                reason: SkipReason::LatchHeld,
                ..
            }
        ));

        // 7. Failed (backend send error)
        let fail_backend = Arc::new(
            ScriptedBackend::new(vec![DIALOG_A.to_string()]).with_send_error("tmux pipe broke"),
        );
        let fail_ctx = make_ctx(fail_backend);
        let s7 = verified_send(
            &fail_ctx,
            Expectation::DialogSummary("Bash command — Do you want to proceed?".into()),
            Payload::Keys(vec!["1".into()]),
            test_seed("lane-8"),
        )
        .await;
        assert!(matches!(s7, SendOutcome::Failed { .. }));

        // 8. Held
        let held_id = record_hold(&ctx, test_seed("lane-7")).await;
        assert!(held_id > 0);

        let logs = ctx.store.supervision_log(None, 20, None).await.unwrap();
        assert_eq!(logs.len(), 7);
        for entry in &logs {
            assert!(!entry.at.to_rfc3339().is_empty());
            assert!(!entry.trigger.is_empty());
            assert!(!entry.decision.is_empty());
            assert!(!entry.outcome.is_empty());
        }

        let fail_logs = fail_ctx
            .store
            .supervision_log(None, 10, None)
            .await
            .unwrap();
        assert_eq!(fail_logs.len(), 1);
        assert_eq!(fail_logs[0].outcome, "failed");
    }

    #[tokio::test]
    async fn line_payload_uses_send_text_and_records_enter() {
        let backend = Arc::new(ScriptedBackend::new(vec![IDLE_PANE.to_string()]));
        let ctx = make_ctx(backend.clone());

        let expect = Expectation::IdleNoDialog;
        let payload = Payload::Line("cargo build".into());
        let seed = test_seed("lane-1");

        let outcome = verified_send(&ctx, expect, payload, seed).await;
        match outcome {
            SendOutcome::Sent { keys, entry_id } => {
                assert_eq!(keys, vec!["<literal:cargo build>", "Enter"]);
                assert!(entry_id > 0);
            }
            other => panic!("expected Sent, got {:?}", other),
        }

        let sent = backend.sent_text.lock().unwrap().clone();
        assert_eq!(
            sent,
            vec![("lane-1".to_string(), "cargo build".to_string())]
        );

        let log = ctx.store.supervision_log(None, 10, None).await.unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].outcome, "sent");
        assert_eq!(
            log[0].keys,
            Some(vec!["<literal:cargo build>".into(), "Enter".into()])
        );
    }
}
