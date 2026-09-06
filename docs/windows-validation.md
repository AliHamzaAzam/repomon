# Windows end-to-end validation

Run the three checklist rounds on a physical or full-VM Windows 11 machine with Windows Terminal, Git for Windows and native Claude Code installed. This is the remaining interactive release gate; Windows binaries already ship, but Windows remains less validated than macOS/Linux until this passes.

**You are here:** the manual release-validation gate. Use a disposable Windows machine and record every result.

## Find your next task

| Task | Go to |
|---|---|
| Prepare the test machine | [Open section](#prepare-the-test-machine) |
| What CI now covers | [Open section](#what-ci-now-covers) |
| Self-contained binaries (no Visual C++ redistributable) | [Open section](#self-contained-binaries-no-visual-c-redistributable) |
| Check installation and diagnostics | [Open section](#check-installation-and-diagnostics) |
| Check durable agent sessions | [Open section](#check-durable-agent-sessions) |
| Check usage, clipboard and remote controls | [Open section](#check-usage-clipboard-and-remote-controls) |
| After it passes | [Open section](#after-it-passes) |

## Prepare the test machine

Use a release candidate from `main` or the existing release installer. **Time: 5-10 minutes for an installer; allow 20-40 minutes for a first source build.**

1. Install the required tools on the Windows 11 test machine.
2. Install the candidate, or build from the repository root with an explicit target.

   ```powershell
   cargo build --release --target x86_64-pc-windows-msvc
   ```

3. Keep `repomon.exe`, `repomond.exe` and `repomon-agent-host.exe` in the same directory.

**You know it worked when:** all three executables are present and `repomond.exe --version` prints a version.

<details>
<summary>Details: current CI status</summary>

The native Windows port is code-complete and CI-green on `x86_64-pc-windows-msvc` (build, `cargo
fmt`, clippy, and the workspace test suite, with the tmux-only tests self-skipping and the
Windows host/backend integration tests running).

CI runs on a fresh GitHub `windows-latest` runner, which cannot exercise the interactive,
durable, multi-process behavior that defines repomon.

</details>

## What CI now covers

Inspect the `windows-smoke` result before starting the manual rounds.

<details>
<summary>Details</summary>

`.github/scripts/windows-smoke.ps1` runs in a `windows-smoke` job that the publishing jobs of
`desktop-preview.yml` and `desktop-release.yml` depend on (and in the manual
`windows-artifact.yml`), so a failure stops the release and the updater feed from being touched.

It builds an unpublished bundle and fails on any of four checks: `repomond.exe --version` runs
(the one that catches a loader failure such as `0xC0000135`), the daemon binds a throwaway named
pipe with a throwaway `REPOMON_DATA_DIR` and `repomon.exe` reads from it, the daemon shuts down
when asked, and the NSIS installer run with `/S` leaves the three executables in one directory.

It touches no real data directory, no real pipe name, and no process it did not start itself.

That covers "can these binaries run and talk to each other at all" on a clean runner.

It does not cover anything in the checklist below, which is interactive, durable, and
multi-process by nature.

</details>

## Self-contained binaries (no Visual C++ redistributable)

Check the installed daemon on a fresh Windows image with no redistributable. **Time: 2 minutes after the VM is ready.**

1. Inspect its dependencies from a Visual Studio developer command prompt.

   ```powershell
   dumpbin /dependents repomond.exe
   ```

2. Run the daemon’s version check.

   ```powershell
   repomond.exe --version
   ```

**You know it worked when:** dependencies include neither `VCRUNTIME140.dll` nor `MSVCP140.dll`, and the version command prints a version.

<details>
<summary>Details</summary>

`repomond.exe`, `repomon-agent-host.exe`, and `repomon.exe` are linked against a **static** C
runtime.

`.cargo/config.toml` sets

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]

[target.aarch64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

Without it every MSVC binary loads `VCRUNTIME140.dll` at process start.

On a machine that has never had a Visual Studio redistributable installed that load fails before
`main` runs and the process exits with `STATUS_DLL_NOT_FOUND` (`0xC0000135`).

The desktop app spawns the daemon detached and windowless, so the only visible symptom was a
connection pill stuck on "Retrying".

Two things to know about the flag:

- **The Tauri app exe builds with it too.** `repomon-desktop` is compiled for the same triple, so
  the whole bundle (app plus the three sidecars) carries its own CRT.
- **Always build with an explicit `--target`.** Flags under `[target.<triple>]` also reach build
  scripts and proc-macro dylibs when the build is not explicitly targeted, and a proc macro
  cannot be linked with a static CRT. The desktop workflows, the Windows leg of `ci.yml`,
  `windows-artifact.yml`, and `apps/desktop/scripts/prepare-sidecar.ts` all pass `--target`, so
  the only way to hit this is a bare `cargo build` on a Windows host. Any new Windows job must
  pass it too.

Verify on the VM after installing a bundle: `dumpbin /dependents repomond.exe` must not list
`VCRUNTIME140.dll` or `MSVCP140.dll`, and `repomond.exe --version` must print a version on a
freshly imaged Windows install with no redistributable.

</details>

## Check installation and diagnostics

Run each check on the disposable Windows test machine and record pass or fail. **Time: about 10-15 minutes per round; agent responses and sign-in add waiting time.**

1. **Brand and window chrome.** The installer, Start menu, taskbar, and app window show the new
   graphite/orange mark.

   Exactly one title bar is visible; minimize, maximize/restore, dragging, and close work.
2. **No VC redistributable needed.** On a Windows image that has never had a Visual C++
   redistributable installed, `repomond.exe --version` prints a version (see "Self-contained
   binaries" above).
3. **Boot diagnostics.** With the daemon deliberately broken (rename `repomond.exe`, or point the
   app at a build without a static CRT on a machine with no redistributable), the connection rail's
   retrying state names the cause, **Show log** opens `repomond.out.log`, and **Copy diagnostics**
   produces a block with the pipe name, the daemon path, and the log tail.

   **Settings > System > Bundled Daemon** reports "Does not start" with the same cause.
4. **CLI install from the app.** **Settings > System > Command-line tools > Install** copies the
   three exes into `%LOCALAPPDATA%\repomon\bin`, adds that directory to `HKCU\Environment`'s `Path`
   (and **not** to the machine PATH), and the card reports the version it read back.

   A **newly opened** terminal runs `repomon --version`.

   **Remove** takes both the files and the PATH entry away.

   Check that a `Path` containing `%USERPROFILE%` still expands afterwards (the value must stay
   `REG_EXPAND_SZ`).
5. **Install / boot.** Run `install.ps1` or put a source build on PATH, then launch `repomon`.

   The daemon auto-spawns over the named pipe and the Fleet view renders.

   Repo/lane CRUD works with no agents yet.

**You know it worked when:** every expected result in this round is observed and recorded.

## Check durable agent sessions

Run each check on the disposable Windows test machine and record pass or fail. **Time: about 10-15 minutes per round; agent responses and sign-in add waiting time.**

1. **Service install.** `repomon daemon install` registers a **logon task** via Task Scheduler; the
   daemon comes back after a sign-out/sign-in.

   `repomon daemon status` reports it; `repomon daemon uninstall` removes it.
2. **Spawn a Claude agent + needs-you cycle.** Add a repo, create a lane (worktree under
   `C:\Users\<u>\code\...`), then spawn a Claude Code agent.

   It reaches Running, then a permission/end-of-turn prompt floats it to the top as **needs you**;
   answering it clears the flag.
3. **Durability / re-adoption.** Kill `repomond.exe` while the agent is mid-work.

   Relaunch the TUI (or let it auto-start the daemon). The agent is **re-adopted, still alive,
   with scrollback intact**.

   Confirm a hand-killed host's registry entry is GC'd on the next scan.
4. **Focus view + input.** The embedded focus view renders the live agent from the host's
   server-side terminal; typing (`i`) reaches the agent.
5. **Pop-out attach + detach.** `↵`/`→`/`a` opens the agent in a new Windows Terminal tab (`repomon
   attach-host`).

   Typing in the tab and the embedded view stay consistent; an alternate-screen TUI (Claude Code)
   renders correctly.

   **`F12` detaches** and leaves the agent running.

**You know it worked when:** every expected result in this round is observed and recorded.

## Check usage, clipboard and remote controls

Run each check on the disposable Windows test machine and record pass or fail. **Time: about 10-15 minutes per round; agent responses and sign-in add waiting time.**

1. **Usage settings.** Settings > Usage edits and resets model rates without a restart.

   Toggle today's sidebar cost off and on, then relaunch to verify persistence.

   Recount progress clears when old sources have been processed, including missing sources.
2. **Clipboard + image paste.** Copy from a pane lands on the Windows clipboard (`Set-Clipboard`);
   `Get-Clipboard` paste works; image paste (`v`) saves the clipboard image to a temp PNG and
   inserts its path.
3. **Toast on needs-you.** A `needs-you` transition fires a Windows toast notification when the TUI
   is not already looking at that agent (including with the TUI closed).
4. **Shell-init cd-on-exit.** `repomon shell-init powershell | Out-String | Invoke-Expression` in
   `$PROFILE`; pressing `c` on a lane exits repomon and `cd`s the PowerShell session into that
   worktree (via the `REPOMON_CD_FILE` temp file).
5. **iOS pairing against the Windows daemon.** Enable the remote bridge, pair the iOS companion app
   against the Windows daemon's tailnet address.

   The JSON-RPC protocol is unchanged across the transport swap, so fleet view, live conversations,
   and the Approve button should just work.

**You know it worked when:** every expected result in this round is observed and recorded.

## After it passes

Record the evidence before approving the next release. **Time: 5 minutes to record results, plus a clean-VM installer test and the separate signing work.**

1. Record the release candidate, OS version, and checklist results, then test the published
   `install.ps1` one-liner on a clean VM before the next release is approved.
2. Code-sign the three binaries so SmartScreen stops warning; drop the unsigned-binary note from
   the README once signing ships.

**You know it worked when:** the candidate, OS and results are recorded; the clean-VM installer passes; the README unsigned-binary note is removed only after signing ships.
