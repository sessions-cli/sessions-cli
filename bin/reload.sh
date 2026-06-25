#!/usr/bin/env bash
# Rebuild/install sessions and refresh daemon + sidebar only.
# Does NOT kill the agents tmux session or workspace windows.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

HOME_DIR="$(sessions_home)"
SESSIONS="$(sessions_binary "$HOME_DIR")"
SOCKET="${HOME_DIR}/.local/run/sessionsd.sock"
SCRIPTS_DIR="$(sessions_scripts_dir "$HOME_DIR")"

echo "== install =="
"${ROOT}/bin/dev-install.sh"

echo ""
echo "== restart daemon =="
if pgrep -f "${SESSIONS} daemon" >/dev/null 2>&1; then
  pkill -f "${SESSIONS} daemon" || true
  sleep 0.3
fi
rm -f "$SOCKET"
"${ROOT}/bin/start-sessionsd.sh"
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if "$SESSIONS" status >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done
if ! "$SESSIONS" status >/dev/null 2>&1; then
  echo "sessionsd failed to start" >&2
  exit 1
fi

echo ""
echo "== reconcile daemon snapshot =="
"$SESSIONS" reconcile

echo ""
echo "== refresh tmux clipboard bindings =="
"$SESSIONS" tmux bootstrap >/dev/null 2>&1 || true
if command -v tmux >/dev/null 2>&1 && tmux has-session -t sessions-ui 2>/dev/null; then
  "$SESSIONS" tmux ui bootstrap >/dev/null 2>&1 || true
fi

echo ""
echo "== deploy sidebar helpers =="
mkdir -p "$SCRIPTS_DIR"
install -m 755 "${ROOT}/bin/ensure-sidebar.sh" "${SCRIPTS_DIR}/ensure-sidebar.sh"

echo ""
echo "== restart sidebar =="
"${SCRIPTS_DIR}/ensure-sidebar.sh" 2>/dev/null || true
pkill -f "${SESSIONS} bar" 2>/dev/null || true
pkill -f "${HOME_DIR}/.local/bin/sessions bar" 2>/dev/null || true
sleep 0.2

if command -v tmux >/dev/null 2>&1 && tmux has-session -t sessions-ui 2>/dev/null; then
  tmux respawn-pane -k -t sessions-ui:ui.0 \
    "exec ${HOME_DIR}/.local/bin/sessions bar" 2>/dev/null || true
else
  echo "sessions-ui tmux session missing — run: sessions ui bootstrap" >&2
fi

echo ""
echo "sessions reloaded — tmux agents session left running."
echo "Verify: sessions status"