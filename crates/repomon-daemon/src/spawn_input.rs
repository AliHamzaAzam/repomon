//! Bounded startup input delivery. A missing ready composer never permits a write.

use std::time::{Duration, Instant};

use repomon_core::agent::backend::{CaptureOpts, SessionBackend, SpawnSpec};
use repomon_core::agent::detect_usage_limit;
use repomon_core::agent::prompt::{detect_active_spinner, detect_dialog};
use repomon_core::agent::text::strip_ansi;
use repomon_core::model::AgentKind;

pub(crate) fn task_spec(spec: SpawnSpec, kind: &AgentKind, task: Option<&str>) -> SpawnSpec {
    match (kind, task) {
        (AgentKind::Hermes, _) | (_, None) => spec,
        // Claude's trailing --allowedTools is variadic. Without this separator it consumes
        // the prompt as another allowed tool, leaving the spawned composer empty.
        (AgentKind::ClaudeCode | AgentKind::Codex, Some(task)) => spec.arg("--").arg(task),
        (AgentKind::OpenCode, Some(task)) => spec.arg("--prompt").arg(task),
        (AgentKind::Antigravity, Some(task)) => spec.arg("--prompt-interactive").arg(task),
        (_, Some(task)) => spec.arg(task),
    }
}

fn normalized(text: &str) -> String {
    strip_ansi(text)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

pub(crate) fn contains_head(pane: &str, text: &str) -> bool {
    let head: String = text.trim().chars().take(40).collect();
    !head.is_empty() && normalized(pane).contains(&normalized(&head))
}

fn ready(pane: &str) -> bool {
    let pane = strip_ansi(pane);
    detect_dialog(&pane).is_none()
        && detect_usage_limit(&pane).is_none()
        && detect_active_spinner(&pane).is_none()
        && pane
            .lines()
            .rev()
            .take(6)
            .map(str::trim)
            .find(|line| line.starts_with(['❯', '>', '›']))
            .is_some_and(|line| matches!(line, "❯" | ">" | "›"))
}

trait Input {
    fn capture(&self, visible: bool) -> Result<String, String>;
    fn paste(&self, text: &str) -> Result<(), String>;
    fn enter(&self) -> Result<(), String>;
}

struct Pane<'a> {
    backend: &'a dyn SessionBackend,
    window: &'a str,
}

impl Input for Pane<'_> {
    fn capture(&self, visible: bool) -> Result<String, String> {
        self.backend
            .capture_named(
                self.window,
                if visible {
                    CaptureOpts::visible()
                } else {
                    CaptureOpts::last(2000)
                },
            )
            .map_err(|e| e.to_string())
    }
    fn paste(&self, text: &str) -> Result<(), String> {
        self.backend
            .paste_text_named(self.window, text)
            .map_err(|e| e.to_string())
    }
    fn enter(&self) -> Result<(), String> {
        self.backend
            .send_key_named(self.window, "Enter")
            .map_err(|e| e.to_string())
    }
}

fn wait_for(
    input: &impl Input,
    deadline: Instant,
    visible: bool,
    predicate: impl Fn(&str) -> bool,
) -> Result<(), String> {
    loop {
        if Instant::now() >= deadline {
            return Err("timed out waiting for the composer or task confirmation".into());
        }
        if predicate(&input.capture(visible)?) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for the composer or task confirmation".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn deliver(
    input: &impl Input,
    text: &str,
    deadline: Instant,
    attempts: usize,
) -> Result<(), String> {
    // Verify before Enter as well as after it. If a paste vanished entirely, retry once, but
    // never concatenate another task onto a partially filled composer or a running turn.
    for attempt in 0..attempts {
        if Instant::now() >= deadline {
            return Err("composer readiness deadline expired; input was not sent".into());
        }
        wait_for(input, deadline, true, ready)?;
        input.paste(text)?;
        let verification_deadline = deadline.min(Instant::now() + Duration::from_millis(500));
        if wait_for(input, verification_deadline, false, |pane| {
            contains_head(pane, text)
        })
        .is_ok()
        {
            input.enter()?;
            return wait_for(input, deadline, false, |pane| contains_head(pane, text));
        }
        if attempt + 1 == attempts || !ready(&input.capture(true)?) {
            return Err("task opening was not visible after paste; input was not submitted".into());
        }
    }
    unreachable!()
}

/// Keep the entire startup check below the client's 15-second RPC timeout. An uncertain
/// launch-argument delivery is retried only at an empty, idle composer, never over a busy turn.
pub(crate) fn finish(
    backend: &dyn SessionBackend,
    window: &str,
    kind: &AgentKind,
    task: Option<&str>,
    effort: Option<&str>,
) -> Vec<String> {
    let input = Pane { backend, window };
    finish_input(
        &input,
        kind,
        task,
        effort,
        Instant::now() + Duration::from_secs(8),
    )
}

fn finish_input(
    input: &impl Input,
    kind: &AgentKind,
    task: Option<&str>,
    effort: Option<&str>,
    deadline: Instant,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(task) = task {
        let result = if matches!(kind, AgentKind::Hermes) {
            deliver(input, task, deadline, 2)
        } else {
            let verification_deadline = deadline.min(Instant::now() + Duration::from_secs(3));
            wait_for(input, verification_deadline, false, |pane| {
                contains_head(pane, task)
            })
            .or_else(|_| {
                // One recovery attempt, using the same readiness and paste verification rules.
                deliver(input, task, deadline, 1)
            })
        };
        if let Err(error) = result {
            warnings.push(format!("Agent started, but initial task delivery could not be verified: {error}. Inspect the agent before resending the task."));
        }
    }
    if let Some(effort) = effort {
        if !warnings.is_empty() {
            warnings.push(format!(
                "{effort} was not sent because task delivery is uncertain."
            ));
            return warnings;
        }
        if let Err(error) = deliver(input, effort, deadline, 2) {
            warnings.push(format!("Agent started, but {effort} was not confirmed: {error}. The initial task uses the launch effort; apply this setting when the agent is idle."));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Scripted {
        ready_at: Instant,
        text: RefCell<String>,
        pastes: Cell<usize>,
        enters: Cell<usize>,
        drop_pastes: usize,
    }
    impl Scripted {
        fn new(delay: Duration, drop_pastes: usize) -> Self {
            Self {
                ready_at: Instant::now() + delay,
                text: RefCell::new(String::new()),
                pastes: Cell::new(0),
                enters: Cell::new(0),
                drop_pastes,
            }
        }
    }
    impl Input for Scripted {
        fn capture(&self, _: bool) -> Result<String, String> {
            if Instant::now() < self.ready_at {
                return Ok("Loading tools".into());
            }
            Ok(format!("❯ {}", self.text.borrow()))
        }
        fn paste(&self, text: &str) -> Result<(), String> {
            self.pastes.set(self.pastes.get() + 1);
            if Instant::now() < self.ready_at {
                // A startup reader consumes the opening bytes instead of the composer.
                self.text.replace(text.chars().skip(1000).collect());
            } else if self.pastes.get() > self.drop_pastes {
                self.text.replace(text.into());
            }
            Ok(())
        }
        fn enter(&self) -> Result<(), String> {
            self.enters.set(self.enters.get() + 1);
            Ok(())
        }
    }
    fn task() -> String {
        format!(
            "FIRST LINE: preserve the complete task.\n\n{}\nFINAL LINE",
            "Quoted 'task' $value `literal` 🦀\n".repeat(70)
        )
    }

    #[test]
    fn slow_startup_reproduces_missing_head_and_readiness_wait_preserves_it() {
        let task = task();
        assert!(task.len() > 2048);
        let legacy = Scripted::new(Duration::from_millis(50), 0);
        legacy.paste(&task).unwrap();
        assert!(!legacy.text.borrow().starts_with("FIRST LINE"));
        assert!(legacy.text.borrow().ends_with("FINAL LINE"));
        let fixed = Scripted::new(Duration::from_millis(50), 0);
        deliver(&fixed, &task, Instant::now() + Duration::from_secs(1), 2).unwrap();
        assert_eq!(*fixed.text.borrow(), task);
        assert_eq!(fixed.pastes.get(), 1);
        assert_eq!(fixed.enters.get(), 1);
    }

    #[test]
    fn readiness_timeout_never_sends() {
        let input = Scripted::new(Duration::from_secs(60), 0);
        assert!(
            deliver(
                &input,
                &task(),
                Instant::now() + Duration::from_millis(10),
                2
            )
            .is_err()
        );
        assert_eq!(input.pastes.get(), 0);
        assert_eq!(input.enters.get(), 0);
    }

    #[test]
    fn lost_paste_is_retried_once_and_verified_before_submit() {
        let input = Scripted::new(Duration::ZERO, 1);
        deliver(&input, &task(), Instant::now() + Duration::from_secs(2), 2).unwrap();
        assert_eq!(input.pastes.get(), 2);
        assert_eq!(input.enters.get(), 1);
        assert_eq!(*input.text.borrow(), task());
    }

    #[test]
    fn persistent_verification_failure_does_not_submit() {
        let input = Scripted::new(Duration::ZERO, 2);
        let error =
            deliver(&input, &task(), Instant::now() + Duration::from_secs(2), 2).unwrap_err();
        assert!(error.contains("not visible"));
        assert_eq!(input.pastes.get(), 2);
        assert_eq!(input.enters.get(), 0);
    }

    #[test]
    fn failed_delivery_returns_a_spawn_warning_and_skips_effort() {
        let input = Scripted::new(Duration::from_secs(60), 0);
        let warnings = finish_input(
            &input,
            &AgentKind::Hermes,
            Some(&task()),
            Some("/effort ultracode"),
            Instant::now() + Duration::from_millis(10),
        );
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("initial task delivery could not be verified"));
        assert!(warnings[1].contains("was not sent"));
        assert_eq!(input.pastes.get(), 0);
    }

    #[test]
    fn confirmed_argument_delivery_is_not_pasted_again() {
        let input = Scripted::new(Duration::ZERO, 0);
        input.text.replace(task());
        assert!(
            finish_input(
                &input,
                &AgentKind::ClaudeCode,
                Some(&task()),
                None,
                Instant::now() + Duration::from_secs(1)
            )
            .is_empty()
        );
        assert_eq!(input.pastes.get(), 0);
    }

    #[test]
    fn readiness_rejects_busy_or_nonempty_composers() {
        assert!(ready("Hermes\n─ ready │ model\n❯ "));
        assert!(ready("Claude Code\n❯ \n? for shortcuts"));
        assert!(!ready("❯ unfinished task"));
        assert!(!ready("Loading tools"));
        assert!(!ready("❯\nesc to interrupt"));
    }

    #[test]
    fn verification_handles_unicode_ansi_and_wrapping() {
        assert!(contains_head(
            "\x1b[32mFIRST LINE: preserve\n the complete task.\x1b[0m",
            &task()
        ));
        assert!(!contains_head("only the tail arrived", &task()));
    }

    #[test]
    fn long_launch_arguments_are_preserved_for_claude_and_codex() {
        for kind in [AgentKind::ClaudeCode, AgentKind::Codex] {
            let spec = task_spec(SpawnSpec::new("agent", "/tmp"), &kind, Some(&task()));
            assert_eq!(spec.args, vec!["--".to_string(), task()]);
        }
        assert!(
            task_spec(
                SpawnSpec::new("hermes", "/tmp"),
                &AgentKind::Hermes,
                Some(&task())
            )
            .args
            .is_empty()
        );
    }
}
