#!/usr/bin/env bash
# Record a 90-second Repomon showcase using eight sandbox lanes and only mock agents.
# --dry-run verifies the fleet, ledger, mail and supervision, launches the isolated app,
# and cleans up without screen capture. Add --tour to rehearse the same AppleScript tour.
# --still writes the GIF's opening frame to docs/preview.png. Both capture modes crop
# the app window to 1440x900 content, excluding the extra bottom band macOS can return.
# The operator runs captures from a terminal with Screen Recording permission.
# No builds, real agents, production socket connections, or personal data are used.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
exec python3 "$SCRIPT_DIR/gui-demo/showcase.py" "$@"
