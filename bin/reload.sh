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
echo "== deploy sidebar helpers =="
mkdir -p "$SCRIPTS_DIR"
install -m 755 "${ROOT}/bin/ensure-sidebar.sh" "${SCRIPTS_DIR}/ensure-sidebar.sh"
install -m 755 "${ROOT}/bin/restart-sidebar.sh" "${SCRIPTS_DIR}/restart-sidebar.sh"
install -m 755 "${ROOT}/bin/verify-ui-runtime.sh" "${SCRIPTS_DIR}/verify-ui-runtime.sh"
install -m 755 "${ROOT}/bin/ensure-skillshare.sh" "${SCRIPTS_DIR}/ensure-skillshare.sh"
install -m 755 "${ROOT}/bin/ensure-obot.sh" "${SCRIPTS_DIR}/ensure-obot.sh"
install -m 755 "${ROOT}/bin/ensure-companions.sh" "${SCRIPTS_DIR}/ensure-companions.sh"
install -m 644 "${ROOT}/bin/paths.sh" "${SCRIPTS_DIR}/paths.sh"

echo ""
echo "== refresh tmux + restart sidebar =="
if ! command -v tmux >/dev/null 2>&1; then
  echo "tmux not installed — sidebar not restarted" >&2
  exit 1
fi
if ! tmux has-session -t agents 2>/dev/null; then
  echo "agents tmux session missing — run: sessions up" >&2
  exit 1
fi
"$SESSIONS" tmux bootstrap
if ! tmux has-session -t sessions-ui 2>/dev/null; then
  echo "sessions-ui missing — run: sessions up" >&2
  exit 1
fi
"$SESSIONS" tmux ui bootstrap
"${SCRIPTS_DIR}/verify-ui-runtime.sh"

echo ""
echo "sessions reloaded — tmux agents session left running."
echo "Verify: sessions status"
echo "        sessions doctor"