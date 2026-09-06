# Windows end-to-end validation

This is the remaining interactive release gate for Windows. Run it on a physical or full-VM Windows 11 machine with Windows Terminal, Git for Windows and native Claude Code. Windows binaries already ship, but remain less validated than macOS/Linux until this checklist passes; code signing is a separate gate.

## Prepare the test machine

1. Install the required tools and the release candidate from `main` or the existing release installer. For a source build, run this from the repository root:

   ```powershell
   cargo build --release --target x86_64-pc-windows-msvc
   ```

2. Keep `repomon.exe`, `repomond.exe` and `repomon-agent-host.exe` in the same directory. Record pass/fail results for each round below.

## What CI now covers

The native port is code-complete and CI-green on `x86_64-pc-windows-msvc`: build, `cargo fmt`, clippy and workspace tests, including Windows host/backend integration tests. Tmux-only tests self-skip. Fresh GitHub `windows-latest` runners cannot exercise the interactive, durable, multi-process behavior below.

`.github/scripts/windows-smoke.ps1` runs in the `windows-smoke` job required by publishing jobs in `desktop-preview.yml` and `desktop-release.yml`, and in manual `windows-artifact.yml` runs. Failure blocks publication and updater-feed changes. It builds an unpublished bundle and checks:

| Check | Expected behavior |
|---|---|
| Loader | `repomond.exe --version` runs, catching failures such as `0xC0000135`. |
| Transport | The daemon binds a throwaway named pipe with a throwaway `REPOMON_DATA_DIR`; `repomon.exe` reads from it. |
| Shutdown | The daemon stops when asked. |
| Installer | NSIS with `/S` leaves all three executables in one directory. |

The smoke test touches no real data directory, real pipe name or process it did not start. It establishes that the binaries run and communicate, not the manual behaviors below.

## Self-contained binaries (no Visual C++ redistributable)

The three sidecars and the Tauri app (`repomon-desktop`) link a static C runtime. `.cargo/config.toml` applies these flags to both MSVC targets:

```toml
[target.x86_64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
[target.aarch64-pc-windows-msvc]
rustflags = ["-C", "target-feature=+crt-static"]
```

Without static linking, MSVC binaries load `VCRUNTIME140.dll` before `main`. A machine without the Visual Studio redistributable exits with `STATUS_DLL_NOT_FOUND` (`0xC0000135`). Because the app spawns a detached, windowless daemon, the visible symptom was a connection pill stuck on "Retrying".

Always pass an explicit `--target`: otherwise flags under `[target.<triple>]` also reach build scripts and proc-macro dylibs, which cannot link a static CRT. Desktop workflows, the Windows leg of `ci.yml`, `windows-artifact.yml` and `apps/desktop/scripts/prepare-sidecar.ts` already do so; new Windows jobs must too. A bare `cargo build` on Windows can hit this failure.

1. On a fresh Windows image without a redistributable, inspect the installed daemon from a Visual Studio developer command prompt:

   ```powershell
   dumpbin /dependents repomond.exe
   repomond.exe --version
   ```

2. Confirm the dependency list contains neither `VCRUNTIME140.dll` nor `MSVCP140.dll` and the version command runs.

## Check installation and diagnostics

1. **Brand and chrome:** check the graphite/orange mark in the installer, Start menu, taskbar and app window. Verify exactly one title bar, minimize, maximize/restore, dragging and close.
2. **Boot diagnostics:** deliberately break the daemon by renaming `repomond.exe`, or use a build without static CRT on a machine without the redistributable. The retrying rail must name the cause; **Show log** opens `repomond.out.log`; **Copy diagnostics** includes the pipe name, daemon path and log tail. **Settings > System > Bundled Daemon** must report "Does not start" with the same cause.
3. **CLI installation:** **Settings > System > Command-line tools > Install** copies all three executables into `%LOCALAPPDATA%\repomon\bin`, adds it to `HKCU\Environment`'s `Path` (not the machine PATH) and reports the version read back. In a newly opened terminal, run:

   ```powershell
   repomon --version
   ```

4. **CLI removal:** **Remove** deletes both files and the PATH entry. A `Path` containing `%USERPROFILE%` must still expand; its type must remain `REG_EXPAND_SZ`.
5. **Install and boot:** run `install.ps1` or put a source build on PATH, then launch. The daemon must auto-spawn over the named pipe, render Fleet and support repo/lane CRUD without agents:

   ```powershell
   repomon
   ```


## Check durable agent sessions

1. **Logon task:** install the service, sign out/in, and verify the daemon returns. Check status, then uninstall the Task Scheduler logon task:

   ```powershell
   repomon daemon install
   repomon daemon status
   repomon daemon uninstall
   ```

2. **Needs-you cycle:** add a repo, create a lane under `C:\Users\<u>\code\...`, and spawn Claude Code. It must reach Running, then rise as **needs you** on a permission/end-of-turn prompt; answering clears the flag.
3. **Durability:** kill `repomond.exe` mid-work, then relaunch the TUI or let it auto-start the daemon. The agent must be re-adopted alive with scrollback intact. A manually killed host's registry entry must be collected on the next scan.
4. **Embedded input:** the focus view must render the host's server-side terminal, and `i` must send typed input to the agent.
5. **Attach/detach:** Enter, Right arrow or `a` must open a Windows Terminal tab through `repomon attach-host`. Typing in the tab and embedded view must agree; Claude Code's alternate-screen TUI must render correctly. `F12` detaches without stopping the agent.

## Check usage, clipboard and remote controls

1. **Usage:** edit/reset model rates in Settings > Usage without restarting. Toggle today's sidebar cost, relaunch and verify persistence. Recount progress must clear after old sources are processed, including missing sources.
2. **Clipboard:** pane copy must reach the Windows clipboard through `Set-Clipboard`; `Get-Clipboard` paste must work. Image paste (`v`) must save a temporary PNG and insert its path.
3. **Notifications:** a needs-you transition must fire a Windows toast when the TUI is not viewing that agent, including while the TUI is closed.
4. **Shell integration:** add the following to `$PROFILE`. Pressing `c` on a lane must exit Repomon and change PowerShell to its worktree through the `REPOMON_CD_FILE` temporary file.

   ```powershell
   repomon shell-init powershell | Out-String | Invoke-Expression
   ```

5. **iOS pairing:** enable the remote bridge and pair the companion app to the Windows daemon's tailnet address. The unchanged JSON-RPC protocol must support fleet view, live conversations and Approve across the transport change.

## After it passes

1. Record the release candidate, OS version and checklist results. Test the published `install.ps1` one-liner on a clean VM before approving the next release.
2. Code-sign the three binaries to remove SmartScreen warnings. Remove the README's unsigned-binary note only after signing ships.
