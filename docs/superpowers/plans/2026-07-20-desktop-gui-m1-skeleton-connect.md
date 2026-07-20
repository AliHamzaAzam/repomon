# Repomon Desktop M1: Skeleton and Connect

**Status:** completed 2026-07-20
**Branch:** `codex/desktop-m1`
**Source:** `docs/superpowers/specs/2026-07-20-desktop-gui-design.md`

## Outcome

M1 delivers a runnable Tauri 2 desktop shell under `apps/desktop/`. The shell starts without
waiting for the daemon, connects to an existing daemon or launches `repomond`, keeps supervising
the connection, and shows live `daemon.status` data in a persistent footer. The root Rust
workspace and CI include the new crate and frontend.

## Locked boundaries

- Keep JSON-RPC in Rust. The frontend receives typed connection snapshots and events only.
- Use `repomon_core::launch` for endpoint resolution and daemon startup.
- Keep `AppState.client` as a `tokio::sync::OnceCell<DaemonClient>`.
- Do not depend on `repomon-tui` from the desktop crate.
- Do not implement the general `daemon_call` or subscription pump until M2.
- Do not add Windows-specific desktop code.
- Preserve all existing untracked workspace files.

## Visual plan

Repomon Desktop is an operator watchfloor, not a generic admin dashboard.

- **Palette:** Ink `#101318`, slate `#181D25`, porcelain `#F3F5F7`, signal cyan `#55D6BE`,
  attention amber `#F3B562`, and fault coral `#F07167`.
- **Type:** platform UI sans for navigation and a platform monospace stack for telemetry.
- **Layout:** a narrow fleet rail, a large terminal bay, a slim repomind rail, and a persistent
  bottom connection strip. M1 renders honest empty states for the future domains.
- **Signature:** the bottom connection strip reads like a hardware status rail. Its leading light,
  endpoint, daemon version, uptime, repo count, and lane count update as one live instrument.
- **Motion:** only the connection light breathes while connecting or retrying. Reduced-motion
  users receive a static indicator.

```text
+----------------+--------------------------------------+----------------+
| FLEET          | TERMINAL BAY                         | REPOMIND       |
|                |                                      |                |
| Connect state  | Waiting for first lane in M2/M3      | Available M5   |
|                |                                      |                |
+----------------+--------------------------------------+----------------+
| ● connected    endpoint     v0.5.0     12 repos / 18 lanes     4h 12m |
+------------------------------------------------------------------------+
```

## Task 1: Lift daemon launch into repomon-core

**Files**

- Create `crates/repomon-core/src/launch.rs`
- Modify `crates/repomon-core/src/lib.rs`
- Modify `crates/repomon-tui/src/lib.rs`

**TDD sequence**

1. Add a core test that binds a temporary Unix socket after a short delay and proves
   `connect_with_retry` survives the startup gap.
2. Run `cargo test -p repomon-core launch::tests::connects_after_startup_gap -- --exact` and
   confirm the test fails because `launch` does not exist.
3. Move `ensure_daemon`, `spawn_daemon`, and `connect_with_retry` into the new core module.
4. Re-export the three functions from `repomon-tui` so existing callers do not churn.
5. Run the focused core test, the TUI library tests, and `cargo fmt --all --check`.

**Acceptance**

- The retry test passes.
- `rg "fn ensure_daemon" crates` finds one implementation in core.
- Existing TUI call sites compile unchanged.

## Task 2: Scaffold the Solid, Tailwind, and Tauri application

**Files**

- Create `apps/desktop/package.json`, `bun.lock`, Vite and TypeScript configuration
- Create `apps/desktop/index.html`
- Create `apps/desktop/src/`
- Create `apps/desktop/src-tauri/`
- Modify root `Cargo.toml` and `.gitignore`

**TDD sequence**

1. Scaffold with `create-tauri-app` using Solid, TypeScript, and bun.
2. Pin the approved major versions and add Vitest with a jsdom environment.
3. Add a smoke test asserting the shell title, honest empty states, and connection rail exist.
4. Run `bun run test` and confirm it fails before the application shell is implemented.
5. Configure Tailwind through `@tailwindcss/vite` with semantic CSS variables at root,
   `@theme inline`, one `@layer base`, and no `tailwind.config.*`.
6. Implement the watchfloor shell and light/dark/system theme persistence.
7. Run `bun run check`, `bun test`, and `bun run build`.

**Acceptance**

- The frontend smoke test passes in jsdom.
- Both dark and light semantic tokens are present.
- The production frontend build succeeds.
- `apps/desktop/src-tauri` is a workspace member named `repomon-desktop`.

## Task 3: Add asynchronous connection supervision

**Files**

- Create `apps/desktop/src-tauri/src/state.rs`
- Create `apps/desktop/src-tauri/src/connection.rs`
- Modify `apps/desktop/src-tauri/src/lib.rs`
- Create `apps/desktop/src/ipc/connection.ts`
- Modify `apps/desktop/src/App.tsx`

**Connection state**

```text
starting -> connecting -> connected
                    |          |
                    v          v
                  retrying <- lost
```

Each snapshot contains a stable phase, optional message, resolved endpoint, and optional daemon
status. Connected snapshots include `uptime_secs`, `repos`, `lanes`, `db_size_bytes`, and
`version` from `daemon.status`.

**TDD sequence**

1. Add Rust reducer tests for connecting, connected, and retrying snapshots.
2. Add a host test using a fake framed socket to prove `daemon.status` maps into a connected
   snapshot without blocking application setup.
3. Run the focused Rust tests and confirm they fail before the supervisor exists.
4. Implement `AppState`, the `connection_status` command, and the setup-spawned supervisor.
5. Add a frontend adapter test using Tauri `mockIPC` for the initial status snapshot.
6. Add an application test that feeds connecting, connected, and retrying snapshots and checks
   connection rail copy and metrics.
7. Implement the Solid connection resource and `connection-state` listener with cleanup.
8. Run focused Rust and frontend tests.

**Acceptance**

- Tauri setup returns immediately while connection work runs on the async runtime.
- An existing daemon wins.
- If no daemon is reachable, core launch starts `repomond` and retries.
- If a connected daemon disappears, the UI enters retrying, launch reasserts daemon startup, and
  the original client reconnects after the socket returns.
- The footer shows the actual daemon version, uptime, repo count, and lane count.

## Task 4: Add CI gates

**Files**

- Modify `.github/workflows/ci.yml`

**TDD sequence**

1. Add the Linux WebKitGTK, GTK, SVG, appindicator, and tmux packages to one apt install step.
2. Add a frontend job with setup-bun, frozen install, TypeScript check, Vitest, and Vite build.
3. Validate the workflow structure locally by inspection and run every command used by the new
   frontend job.
4. Run the existing Rust gates locally.

**Acceptance**

- `cargo fmt --all --check` passes.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` passes.
- `cargo test --workspace --locked` passes.
- `bun install --frozen-lockfile`, `bun run check`, `bun run test`, and `bun run build` pass in
  `apps/desktop/`.

## Task 5: Live milestone verification and commits

1. Build `repomond` and the desktop app.
2. Start the desktop app against the configured local endpoint.
3. Confirm the rail reports the live daemon version and counts.
4. Stop only the daemon created for this verification, leaving tmux agents untouched.
5. Confirm the UI enters retrying and returns to connected after `repomond` restarts.
6. Capture the final git diff and verify no unrelated untracked files are staged.
7. Commit bite-sized changes as **Ali Hamza Azam** with no em dash in commit messages.

## M1 handoff gate

Pause after M1 with:

- a summary of delivered behavior;
- verification commands and results;
- the live retry observation;
- any deferred issues that belong to M2;
- a clean list of committed files and untouched pre-existing untracked files.

## Verification record

- Frontend: frozen install, TypeScript check, 4 Vitest tests, and Vite 7 production build pass.
- Rust: workspace fmt, clippy with warnings denied, and the full locked workspace test suite pass.
- Host: the focused framed-socket test maps `daemon.status` into a connected snapshot.
- Live: the desktop started an isolated daemon on its private socket. Stopping PID 12515 moved
  through the recovery path and launched PID 30137 on the same socket. The replacement reported
  version 0.5.0 with fresh uptime through `daemon status`.
- Isolation: the verification app, daemon, socket, database, logs, and temporary directory were
  removed after the smoke test. The live fleet was not touched.
