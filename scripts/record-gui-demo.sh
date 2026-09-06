#!/usr/bin/env bash
# Records an isolated mock-agent showcase; captures require Screen Recording permission. --dry-run
# verifies and cleans up without capture, --tour rehearses navigation, and --still writes
# docs/preview.png.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
exec python3 "$SCRIPT_DIR/gui-demo/showcase.py" "$@"
