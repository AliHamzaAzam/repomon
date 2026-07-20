# Desktop end-to-end test

`isolated.sh` follows the repository verify protocol: it creates a private config home, data
directory, Unix socket, git fixture, and tmux server name. It never connects to the default daemon
or the live `repomon` tmux server.

On Linux, install `tauri-driver`, WebKitWebDriver, tmux, and Xvfb, build the debug desktop binary,
then run:

```sh
xvfb-run -a bun run e2e
```

The test waits for the connected mission-control UI, confirms the registered fixture lane, opens
an interactive shell tile, sends a command through xterm, verifies the streamed output, and opens
the control center. Cleanup stops the private daemon and tmux server and removes its temporary root.
