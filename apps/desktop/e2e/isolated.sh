#!/usr/bin/env bash
set -euo pipefail

desktop_dir="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$desktop_dir/../.." && pwd)"
run_root="$(mktemp -d "${TMPDIR:-/tmp}/repomon-desktop-e2e.XXXXXX")"
config_home="$run_root/config"
data_dir="$run_root/data"
fixture_repo="$run_root/fixture-repo"
socket_path="$run_root/repomon.sock"
tmux_server="desktop-e2e-$$"
driver_pid=""
daemon_pid=""

desktop_bin="${REPOMON_DESKTOP_BIN:-$repo_root/target/debug/repomon-desktop}"
cli_bin="${REPOMON_CLI_BIN:-$repo_root/target/debug/repomon}"
daemon_bin="${REPOMON_DAEMON_BIN:-$repo_root/target/debug/repomond}"

cleanup() {
  if [[ -n "$driver_pid" ]]; then kill "$driver_pid" 2>/dev/null || true; fi
  if [[ -n "$daemon_pid" ]]; then kill "$daemon_pid" 2>/dev/null || true; fi
  tmux -L "$tmux_server" kill-server 2>/dev/null || true
  rm -rf "$run_root"
}
trap cleanup EXIT INT TERM

mkdir -p "$config_home/repomon" "$data_dir" "$fixture_repo"
printf 'tmux_session = "%s"\ndefault_agent = "fake"\nspawn_prompt = false\n' "$tmux_server" > "$config_home/repomon/config.toml"

git -C "$fixture_repo" init -b main >/dev/null
git -C "$fixture_repo" config user.name "Repomon E2E"
git -C "$fixture_repo" config user.email "e2e@repomon.local"
printf 'fixture\n' > "$fixture_repo/README.md"
git -C "$fixture_repo" add README.md
git -C "$fixture_repo" commit -m "initial fixture" >/dev/null

export XDG_CONFIG_HOME="$config_home"
export REPOMON_DATA_DIR="$data_dir"
export REPOMON_SOCKET="$socket_path"
export REPOMON_DESKTOP_BIN="$desktop_bin"

"$daemon_bin" --socket "$socket_path" --data "$data_dir/repomon.db" > "$run_root/daemon.log" 2>&1 &
daemon_pid="$!"
for _ in $(seq 1 100); do
  [[ -S "$socket_path" ]] && break
  sleep 0.05
done
[[ -S "$socket_path" ]] || { cat "$run_root/daemon.log"; exit 1; }

"$cli_bin" --socket "$socket_path" add "$fixture_repo" >/dev/null

tauri-driver --port 4444 > "$run_root/driver.log" 2>&1 &
driver_pid="$!"
for _ in $(seq 1 100); do
  curl --silent --fail http://127.0.0.1:4444/status >/dev/null 2>&1 && break
  sleep 0.1
done

if ! curl --silent --fail http://127.0.0.1:4444/status >/dev/null 2>&1; then
  cat "$run_root/driver.log"
  exit 1
fi

bun "$desktop_dir/e2e/run.ts"
