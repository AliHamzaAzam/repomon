# Desktop end-to-end test

Run the isolated Linux test from `apps/desktop`. The harness creates its own config, data, Unix socket, Git fixture, Repomind home, basic-memory config and tmux server, and never connects to the default daemon or live tmux server.

## Run the test

Prepare the Linux tools before starting. **Time: 10-20 minutes for first setup and build; about 2 minutes for a warm test run.**

1. Install `tauri-driver`, WebKitWebDriver, tmux and Xvfb on Linux.
2. Install and build the frontend from `apps/desktop`.

   ```sh
   bun install
   bun run build
   ```

3. Build the debug desktop, CLI and daemon binaries from the repository root.

   ```sh
   cargo build -p repomon-desktop -p repomon-tui -p repomon-daemon
   ```

4. Enter the desktop directory and run the test.

   ```sh
   cd apps/desktop
   xvfb-run -a bun run e2e
   ```

**You know it worked when:** the test reports success after checking the connected UI, fixture lane, interactive shell, streamed xterm output and Control Center.

<details>
<summary>Details: isolation and cleanup</summary>

[isolated.sh](isolated.sh) follows the repository verify protocol.

Cleanup stops the private daemon and tmux server and removes the temporary root.

</details>
