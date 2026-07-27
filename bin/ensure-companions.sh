#!/usr/bin/env bash
# Ensure skillshare + Obot companions used by sessions Skills / MCP panels.
# Safe to run at install, login (LaunchAgent), and sessions up / daemon start.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Prefer sibling scripts next to this file (deployed scripts dir or repo bin/).
skillshare_script="${SCRIPT_DIR}/ensure-skillshare.sh"
obot_script="${SCRIPT_DIR}/ensure-obot.sh"
if [[ ! -x "$skillshare_script" && -x "${ROOT}/bin/ensure-skillshare.sh" ]]; then
  skillshare_script="${ROOT}/bin/ensure-skillshare.sh"
fi
if [[ ! -x "$obot_script" && -x "${ROOT}/bin/ensure-obot.sh" ]]; then
  obot_script="${ROOT}/bin/ensure-obot.sh"
fi

QUIET_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --quiet|-q) QUIET_ARGS+=(--quiet) ;;
  esac
done

status=0

if [[ -x "$skillshare_script" ]]; then
  if ! "$skillshare_script" "${QUIET_ARGS[@]+"${QUIET_ARGS[@]}"}"; then
    status=1
  fi
else
  echo "  FAIL companions: ensure-skillshare.sh missing" >&2
  status=1
fi

if [[ -x "$obot_script" ]]; then
  if ! "$obot_script" "${QUIET_ARGS[@]+"${QUIET_ARGS[@]}"}"; then
    status=1
  fi
else
  echo "  FAIL companions: ensure-obot.sh missing" >&2
  status=1
fi

exit "$status"
