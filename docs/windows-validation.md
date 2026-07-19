# Windows end-to-end validation

The native Windows port is code-complete and CI-green on `x86_64-pc-windows-msvc` (build,
`cargo fmt`, clippy, and the workspace test suite, with the tmux-only tests self-skipping and
the Windows host/backend integration tests running). CI runs on a fresh GitHub `windows-latest`
runner, which cannot exercise the interactive, durable, multi-process behavior that defines
repomon.

**This checklist is the remaining manual gate. It requires a physical (or full-VM) Windows 11
machine** with Windows Terminal, Git for Windows, and native Claude Code installed. Until it
passes end to end, no Windows release should be tagged, and the binaries stay unsigned (so
SmartScreen will warn; see the README's Windows platform notes).

Run it against a build from `release/windows-preview` (`cargo build --release`, or `install.ps1`
once a preview zip exists). Keep `repomon.exe`, `repomond.exe`, and `repomon-agent-host.exe` in
the same directory.

## Checklist

- [ ] **Install / boot.** `install.ps1` (or a from-source build on PATH) → `repomon` launches,
      the daemon auto-spawns over the named pipe, the Fleet view renders. Repo/lane CRUD works
      with no agents yet.
- [ ] **Service install.** `repomon daemon install` registers a **logon task** via Task
      Scheduler; the daemon comes back after a sign-out/sign-in. `repomon daemon status` reports
      it; `repomon daemon uninstall` removes it.
- [ ] **Spawn a Claude agent + needs-you cycle.** Add a repo → create a lane (worktree under
      `C:\Users\<u>\code\...`) → spawn a Claude Code agent. It reaches Running, then a
      permission/end-of-turn prompt floats it to the top as **needs you**; answering it clears
      the flag.
- [ ] **Durability / re-adoption.** Kill `repomond.exe` while the agent is mid-work. Relaunch
      the TUI (or let it auto-start the daemon) → the agent is **re-adopted, still alive, with
      scrollback intact**. Confirm a hand-killed host's registry entry is GC'd on the next scan.
- [ ] **Focus view + input.** The embedded focus view renders the live agent from the host's
      server-side terminal; typing (`i`) reaches the agent.
- [ ] **Pop-out attach + detach.** `↵`/`→`/`a` opens the agent in a new Windows Terminal tab
      (`repomon attach-host`). Typing in the tab and the embedded view stay consistent; an
      alternate-screen TUI (Claude Code) renders correctly. **`F12` detaches** and leaves the
      agent running.
- [ ] **Clipboard + image paste.** Copy from a pane lands on the Windows clipboard
      (`Set-Clipboard`); `Get-Clipboard` paste works; image paste (`v`) saves the clipboard
      image to a temp PNG and inserts its path.
- [ ] **Toast on needs-you.** A `needs-you` transition fires a Windows toast notification when
      the TUI is not already looking at that agent (including with the TUI closed).
- [ ] **Shell-init cd-on-exit.** `repomon shell-init powershell | iex` in `$PROFILE`; pressing
      `c` on a lane exits repomon and `cd`s the PowerShell session into that worktree (via the
      `REPOMON_CD_FILE` temp file).
- [ ] **iOS pairing against the Windows daemon.** Enable the remote bridge, pair the iOS
      companion app against the Windows daemon's tailnet address. The JSON-RPC protocol is
      unchanged across the transport swap, so fleet view, live conversations, and the Approve
      button should just work.

## After it passes

1. Tag a Windows release (see "Cutting a Windows release" in [../STATUS.md](../STATUS.md)) and
   test the `install.ps1` one-liner on a clean VM.
2. Code-sign the three binaries so SmartScreen stops warning; drop the unsigned-binary note from
   the README once signing ships.
