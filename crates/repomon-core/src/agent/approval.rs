//! Approval-policy memory primitives: extract a learnable command pattern from a permission
//! dialog, and the hardcoded always-escalate sniffer that no learned rule can override.
//!
//! Only **Bash** permission dialogs learn patterns — an Edit/Write dialog is file-specific and
//! generalizing it would approve edits to arbitrary files. The pattern is deliberately coarse
//! (first two command tokens): "cargo test -p foo" and "cargo test --workspace" are the same
//! human intent, while "cargo publish" is not.

use super::prompt::PendingDialog;

/// The proposed shell command inside a Bash permission dialog, when this dialog is one.
pub fn dialog_command(d: &PendingDialog) -> Option<String> {
    let title = d.title.as_deref()?;
    if !title.trim_start().starts_with("Bash") {
        return None;
    }
    d.body
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Reduce a command to its learnable pattern: the first two whitespace tokens ("cargo test"),
/// or the single token for one-word commands.
pub fn command_pattern(cmd: &str) -> String {
    cmd.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

/// Commands that must reach a human regardless of any learned rule. Matched anywhere in the
/// command text (compound commands like `cd x && git push --force` still trip it).
pub fn is_always_escalate(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    let has = |needle: &str| c.contains(needle);
    // Force pushes: --force/-f attached to a push, however the flags are ordered.
    let force_push = has("push") && (has("--force") || c.split_whitespace().any(|t| t == "-f"));
    let rm_rf = c.split_whitespace().any(|t| t == "rm")
        && c.split_whitespace()
            .any(|t| t.starts_with('-') && t.contains('r') && t.contains('f'));
    let reset_hard = has("reset") && has("--hard");
    let clean_force = has("git clean") && c.split_whitespace().any(|t| t.starts_with("-f"));
    let sudo_rm = has("sudo rm");
    force_push || rm_rf || reset_hard || clean_force || sudo_rm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::prompt::DialogOption;

    fn dialog(title: Option<&str>, body: &[&str]) -> PendingDialog {
        PendingDialog {
            title: title.map(str::to_string),
            question: "Do you want to proceed?".into(),
            body: body.iter().map(|s| s.to_string()).collect(),
            options: vec![
                DialogOption {
                    number: Some(1),
                    text: "Yes".into(),
                },
                DialogOption {
                    number: Some(2),
                    text: "No".into(),
                },
            ],
            selected: Some(0),
        }
    }

    #[test]
    fn bash_dialogs_yield_their_command() {
        let d = dialog(
            Some("Bash command"),
            &["", "cargo test -p repomon-core", ""],
        );
        assert_eq!(
            dialog_command(&d).as_deref(),
            Some("cargo test -p repomon-core")
        );
    }

    #[test]
    fn non_bash_dialogs_never_learn() {
        assert_eq!(
            dialog_command(&dialog(Some("Edit file"), &["src/main.rs"])),
            None
        );
        assert_eq!(dialog_command(&dialog(None, &["anything"])), None);
    }

    #[test]
    fn patterns_are_the_first_two_tokens() {
        assert_eq!(command_pattern("cargo test -p foo --lib"), "cargo test");
        assert_eq!(command_pattern("cargo test --workspace"), "cargo test");
        assert_eq!(command_pattern("ls"), "ls");
        assert_eq!(command_pattern("  git   push origin main "), "git push");
    }

    #[test]
    fn destructive_commands_always_escalate() {
        for cmd in [
            "git push --force",
            "git push -f origin main",
            "git push origin main --force-with-lease",
            "rm -rf /tmp/x",
            "rm -fr build",
            "sudo rm /etc/hosts",
            "git reset --hard HEAD~1",
            "git clean -fd",
            "cd x && git push --force",
        ] {
            assert!(is_always_escalate(cmd), "{cmd} must always escalate");
        }
    }

    #[test]
    fn routine_commands_do_not_escalate() {
        for cmd in [
            "cargo test -p foo",
            "git push origin main",
            "rm build/output.txt",
            "git reset HEAD~1",
            "npm run format",
        ] {
            assert!(!is_always_escalate(cmd), "{cmd} should be learnable");
        }
    }
}
