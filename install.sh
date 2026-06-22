#!/bin/sh
# repomon installer. Downloads prebuilt binaries from GitHub Releases.
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
arch="$(uname -m)"
case "$os" in
  Darwin)
    case "$arch" in
      arm64 | aarch64) target="aarch64-apple-darwin" ;;
      x86_64) target="x86_64-apple-darwin" ;;
      *) echo "unsupported macOS architecture: $arch" >&2; exit 1 ;;
    esac ;;
  Linux)
    case "$arch" in
      x86_64) target="x86_64-unknown-linux-gnu" ;;
      *) echo "no prebuilt binary for Linux $arch. Install from source:" >&2
         echo "  cargo install --git https://github.com/$REPO repomon-tui repomon-daemon" >&2
         exit 1 ;;
    esac ;;
  *)
    echo "unsupported OS: $os. See the README for the from-source install." >&2
    exit 1 ;;
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

# Runtime dependency checks. repomon needs tmux (agents run in it) and git.
install_hint() { # $1 = package; prints the install command for this OS
  case "$os" in
    Darwin) echo "  brew install $1" ;;
    Linux)
      if command -v apt    >/dev/null 2>&1; then echo "  sudo apt install $1"
      elif command -v dnf  >/dev/null 2>&1; then echo "  sudo dnf install $1"
      elif command -v pacman >/dev/null 2>&1; then echo "  sudo pacman -S $1"
      elif command -v zypper >/dev/null 2>&1; then echo "  sudo zypper install $1"
      else echo "  install '$1' with your package manager"; fi ;;
    *) echo "  install '$1' with your package manager" ;;
  esac
}
for dep in tmux git; do
  if ! command -v "$dep" >/dev/null 2>&1; then
    echo "Warning: '$dep' is not installed. repomon needs it. Install it:"
    install_hint "$dep"
  fi
done

echo
echo "Enable cd-on-exit by adding to your ~/.zshrc (or ~/.bashrc):"
echo "    eval \"\$(repomon shell-init zsh)\""
echo "Run 'repomon' to get started."
