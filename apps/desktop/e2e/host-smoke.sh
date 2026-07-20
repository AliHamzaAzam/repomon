#!/usr/bin/env bash
set -euo pipefail

desktop_dir="$(cd "$(dirname "$0")/.." && pwd)"
repo_root="$(cd "$desktop_dir/../.." && pwd)"
run_root="$(mktemp -d "${TMPDIR:-/tmp}/repomon-desktop-host.XXXXXX")"
config_home="$run_root/config"
data_dir="$run_root/data"
fixture_repo="$run_root/fixture-repo"
socket_path="$run_root/repomon.sock"
tmux_server="desktop-host-$$"
daemon_pid=""
desktop_pid=""

desktop_bin="${REPOMON_DESKTOP_BIN:-$repo_root/target/debug/repomon-desktop}"
cli_bin="${REPOMON_CLI_BIN:-$repo_root/target/debug/repomon}"
daemon_bin="${REPOMON_DAEMON_BIN:-$repo_root/target/debug/repomond}"

cleanup() {
  if [[ -n "$desktop_pid" ]]; then kill "$desktop_pid" 2>/dev/null || true; fi
  "$cli_bin" --socket "$socket_path" daemon stop >/dev/null 2>&1 || true
  if [[ -n "$daemon_pid" ]]; then kill "$daemon_pid" 2>/dev/null || true; fi
  tmux -L "$tmux_server" kill-server 2>/dev/null || true
  rm -rf "$run_root"
}
trap cleanup EXIT INT TERM

mkdir -p "$config_home/repomon" "$data_dir" "$fixture_repo"
printf 'tmux_session = "%s"\n' "$tmux_server" > "$config_home/repomon/config.toml"
git -C "$fixture_repo" init -b main >/dev/null
git -C "$fixture_repo" config user.name "Repomon Host Smoke"
git -C "$fixture_repo" config user.email "host-smoke@repomon.local"
printf 'fixture\n' > "$fixture_repo/README.md"
git -C "$fixture_repo" add README.md
git -C "$fixture_repo" commit -m "initial fixture" >/dev/null

export XDG_CONFIG_HOME="$config_home"
export REPOMON_DATA_DIR="$data_dir"
export REPOMON_SOCKET="$socket_path"

"$daemon_bin" --socket "$socket_path" --data "$data_dir/repomon.db" > "$run_root/daemon.log" 2>&1 &
daemon_pid="$!"
for _ in $(seq 1 100); do
  [[ -S "$socket_path" ]] && break
  sleep 0.05
done
"$cli_bin" --socket "$socket_path" add "$fixture_repo" >/dev/null

"$desktop_bin" > "$run_root/desktop.log" 2>&1 &
desktop_pid="$!"
sleep 3
kill "$daemon_pid"
wait "$daemon_pid" 2>/dev/null || true
daemon_pid=""

connected=false
for _ in $(seq 1 80); do
  if "$cli_bin" --socket "$socket_path" --print-once 2>/dev/null | grep -q 'fixture-repo'; then
    connected=true
    break
  fi
  sleep 0.25
done

if [[ "$connected" != true ]]; then
  cat "$run_root/desktop.log"
  exit 1
fi

printf 'desktop host reconnected to an isolated daemon with one repository\n'
if [[ "${REPOMON_SMOKE_HOLD_SECONDS:-0}" != "0" ]]; then
  sleep "$REPOMON_SMOKE_HOLD_SECONDS"
fi
