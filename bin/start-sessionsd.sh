#!/usr/bin/env bash
# Start sessionsd if the socket is not responding.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

HOME_DIR="$(sessions_home)"
SOCKET="${HOME_DIR}/.local/run/sessionsd.sock"
SESSIONS="$(sessions_binary "$HOME_DIR")"
LOG="$(sessions_logs_dir "$HOME_DIR")/sessionsd.log"

if [[ ! -x "$SESSIONS" ]]; then
  echo "sessions binary missing: $SESSIONS" >&2
  exit 1
fi

if [[ -S "$SOCKET" ]]; then
  if "$SESSIONS" status >/dev/null 2>&1; then
    exit 0
  fi
  rm -f "$SOCKET"
fi

mkdir -p "$(dirname "$SOCKET")" "$(dirname "$LOG")"
nohup "$SESSIONS" daemon --foreground >>"$LOG" 2>&1 &
disown 2>/dev/null || true
sleep 0.3
exit 0
