#!/bin/sh
# repomon installer — downloads prebuilt binaries from GitHub Releases.
# No Homebrew, no Rust, no Xcode required.
#
#   curl -fsSL https://github.com/AliHamzaAzam/repomon/releases/latest/download/install.sh | sh
#
# Env overrides:
#   REPOMON_INSTALL_DIR   install location (default: ~/.local/bin)
#   REPOMON_VERSION       version tag to install (default: latest), e.g. v0.1.0
set -eu

REPO="AliHamzaAzam/repomon"
DEST="${REPOMON_INSTALL_DIR:-$HOME/.local/bin}"

os="$(uname -s)"
if [ "$os" != "Darwin" ]; then
  echo "repomon currently supports macOS only (detected: $os)." >&2
  exit 1
fi

case "$(uname -m)" in
  arm64 | aarch64) target="aarch64-apple-darwin" ;;
  x86_64) target="x86_64-apple-darwin" ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

if [ "${REPOMON_VERSION:-latest}" = "latest" ]; then
  tag="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)"
else
  tag="$REPOMON_VERSION"
fi
[ -n "${tag:-}" ] || { echo "could not determine release tag" >&2; exit 1; }
ver="${tag#v}"

url="https://github.com/$REPO/releases/download/$tag/repomon-$ver-$target.tar.gz"
echo "Downloading repomon $ver ($target)…"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fSL --proto '=https' --tlsv1.2 "$url" -o "$tmp/repomon.tar.gz"
tar -xzf "$tmp/repomon.tar.gz" -C "$tmp"

mkdir -p "$DEST"
install -m 0755 "$tmp/repomon" "$tmp/repomond" "$DEST/"
echo "Installed repomon and repomond to $DEST"

# PATH hint
case ":$PATH:" in
  *":$DEST:"*) ;;
  *) echo "Note: $DEST is not on your PATH. Add this to your shell rc:"; echo "    export PATH=\"$DEST:\$PATH\"" ;;
esac

# Runtime dependency checks (we can't install these without a package manager)
for dep in tmux git; do
  command -v "$dep" >/dev/null 2>&1 || \
    echo "Warning: '$dep' not found — repomon needs it (agents run in tmux; git operations use git)."
done

echo
echo "Enable cd-on-exit by adding to your ~/.zshrc (or ~/.bashrc):"
echo "    eval \"\$(repomon shell-init zsh)\""
echo "Run 'repomon' to get started."
