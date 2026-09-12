#!/usr/bin/env bash
# Exercises scripts/setup-shared-cargo-target.sh end to end against throwaway
# directories, never touching the operator's real ~/code layout. Not part of
# the required gates; run by hand or from CI as a check on the mechanism
# itself:
#
#   bash scripts/test-shared-cargo-target.sh
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
setup_script="$script_dir/setup-shared-cargo-target.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# Canonicalize: macOS's mktemp returns a path through the /var symlink, but
# cargo reports target_directory already resolved through /private/var, which
# would make every raw string comparison below spuriously fail.
work="$(cd "$work" && pwd -P)"

fail() {
    echo "FAIL: $1" >&2
    exit 1
}

# --- two lanes of one repo end up pointed at the same, identical target-dir ---
export REPOMON_CARGO_TARGET_ROOT="$work/cache"
lanes_root="$work/code/demo-repo-wt"
mkdir -p "$lanes_root/lane-a" "$lanes_root/lane-b"

"$setup_script" "$lanes_root" >/dev/null
config="$lanes_root/.cargo/config.toml"
[ -f "$config" ] || fail "expected $config to exist"

target_dir="$(sed -n 's/^target-dir = "\(.*\)"$/\1/p' "$config")"
[ -n "$target_dir" ] || fail "could not read target-dir out of $config"
[ "$target_dir" = "$work/cache/demo-repo" ] || fail "unexpected target-dir: $target_dir"

# --- idempotent: re-running produces byte-identical output ---
cp "$config" "$work/config.before"
"$setup_script" "$lanes_root" >/dev/null
cmp -s "$work/config.before" "$config" || fail "re-running the setup script changed the generated file"

# --- a second repo's lanes root gets a distinct target dir ---
lanes_root_2="$work/code/other-repo-wt"
mkdir -p "$lanes_root_2"
"$setup_script" "$lanes_root_2" >/dev/null
target_dir_2="$(sed -n 's/^target-dir = "\(.*\)"$/\1/p' "$lanes_root_2/.cargo/config.toml")"
[ "$target_dir_2" != "$target_dir" ] || fail "two different repos resolved to the same target dir"

# --- cargo actually honors the ancestor config from inside a lane ---
if command -v cargo >/dev/null 2>&1; then
    crate="$lanes_root/lane-a/crate"
    mkdir -p "$crate/src"
    cat >"$crate/Cargo.toml" <<'EOF'
[package]
name = "sharedtargettestcrate"
version = "0.1.0"
edition = "2021"
EOF
    echo 'fn main() {}' >"$crate/src/main.rs"
    resolved="$(cd "$crate" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
        | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
    [ "$resolved" = "$target_dir" ] || fail "cargo resolved target_directory=$resolved, expected $target_dir"
else
    echo "cargo not on PATH; skipped the live cargo-metadata check" >&2
fi

# --- a lane-local override wins over the shared ancestor config (the opt-out) ---
if command -v cargo >/dev/null 2>&1; then
    mkdir -p "$crate/.cargo"
    cat >"$crate/.cargo/config.toml" <<'EOF'
[build]
target-dir = "target"
EOF
    resolved="$(cd "$crate" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
        | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
    [ "$resolved" = "$crate/target" ] || fail "lane-local opt-out did not win: got $resolved"
fi

echo "ok: shared-cargo-target mechanism behaves as documented"
