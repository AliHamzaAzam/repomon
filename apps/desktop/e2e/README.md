# Desktop end-to-end test

The Linux [isolated.sh](isolated.sh) harness follows the repository verify protocol with its own config, data, Unix socket, Git fixture, Repomind home, basic-memory config and tmux server. It never connects to the default daemon or live tmux server. It checks the connected UI, registered lane, interactive shell, streamed xterm output and Control Center; cleanup stops the private daemon/tmux server and removes the temporary root.

## Run the test

1. Install `tauri-driver`, WebKitWebDriver, tmux and Xvfb on Linux.
2. From the repository root, install/build the frontend, build the debug desktop/CLI/daemon, then run the test:

   ```sh
   (cd apps/desktop && bun install && bun run build)
   cargo build -p repomon-desktop -p repomon-tui -p repomon-daemon
   cd apps/desktop
   xvfb-run -a bun run e2e
   ```
