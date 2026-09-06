# Brief W2: Windows-correct System check (onboarding step 2 and Settings > System)

Operator evidence (2026-09-06, screenshot from the Windows VM running the 3ff1de1 installer): the
daemon starts and the wizard's System step is populated, but the "Core runtime dependencies"
card shows `tmux  Missing  Not found on PATH or bundle` with the hint `sudo apt install tmux`,
and `git  Missing` with `sudo apt install git`. Both are wrong on Windows.

Branch `fix/windows-system-check` in a fresh worktree `/private/tmp/repomon-fix-windows-system-check`
from current main (3ff1de1 or later). Rules as always (worktree only, no production socket/DB, no
kill by name, no `git add -A`, no hex/emoji/em-dashes, 1-line Conventional Commits, no co-author
trailer, no merge/push/bundle). Run `/frontend-design` and `/impeccable` before touching the
card markup and keep it in the same visual system as the neighbouring cards.

1. **tmux is not a Windows dependency.** `apps/desktop/src/components/SystemHealthView.tsx`
   (the `tmuxInfo` rows around lines 193-290) treats tmux as required everywhere. On Windows the
   agents run through the bundled ConPTY agent host (`repomon-agent-host.exe`). Make the daemon's
   `system.doctor` result carry the platform and, on Windows, an `agent_host` entry (path,
   version, `source: bundled | path | missing`) instead of a meaningful tmux entry (keep the
   field for compatibility but mark it `not_applicable`); the card then shows a row "Agent host
   (ConPTY)" with Bundled or Missing, and no tmux row. The "all good" summary must not count tmux
   on Windows. ts-rs bindings regenerated. Tests for the doctor result on each platform (pure
   function with an injected platform) and for the card rendering both shapes.
2. **Platform-correct install hints.** `getSystemInstallCommand` returns `brew install` on macOS
   and `sudo apt install` otherwise. Add Windows: git `winget install --id Git.Git -e`. For the
   agent hints in `getAgentInstallInfo`, on Windows use `npm install -g ...` where npm-based
   (works) and, for curl-pipe-bash installers (Antigravity, OpenCode, others), show the vendor's
   Windows instruction or the download page instead of a bash pipeline; if a vendor has no Windows
   install, say so plainly. Derive the platform from one helper (the `isMac()` sibling in
   `keymap.ts`, add `isWindows()`), not from user-agent sniffing in the component. Tests per
   platform.
3. **Git detection on Windows.** Confirm `system.doctor` finds `git.exe` when Git for Windows is
   installed only under `C:\Program Files\Git\cmd` (on PATH for new processes but possibly not for
   the app started from the installer before a re-login): search PATH, then the two standard
   install locations, and report the path. Test on the pure lookup with an injected PATH.
4. **Copy of the CLI card note.** "These tools are copies and will not follow app updates" is the
   Windows/AppImage wording; keep it, but add the one line that says when to redo it ("after an
   app update, Remove and Install again") only on those platforms, not on macOS where symlinks
   follow the bundle.

Gate: `cargo test -p repomon-core -p repomon-daemon -p repomon-tui`; in `apps/desktop` `bun run
check`, `bun run test`, `bun run bindings:check`. Report commit hashes per item, tests, gate
tails, and exactly what still needs the operator's VM.
