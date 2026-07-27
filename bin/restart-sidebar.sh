#!/usr/bin/env bash
# Respawn sessions bar in sessions-ui:ui.0 using the installed binary.
# Prefer: sessions tmux ui bootstrap (also refreshes bindings + verifies runtime).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "${SCRIPT_DIR}/paths.sh" ]]; then
  # shellcheck source=paths.sh
  . "${SCRIPT_DIR}/paths.sh"
  SESSIONS="$(sessions_binary "$(sessions_home)")"
else
  SESSIONS="$(command -v sessions)"
fi

exec "$SESSIONS" tmux ui bootstrap