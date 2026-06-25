#!/usr/bin/env bash
# Verify sessions-cli artifacts are gone after uninstall.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

HOME_DIR="$(sessions_home)"
INSTALL_DIR="$(sessions_install_dir "$HOME_DIR")"
DATA_ROOT="$(sessions_data_root "$HOME_DIR")"
SOCKET="${HOME_DIR}/.local/run/sessionsd.sock"
PATH_SESSIONS="${HOME_DIR}/.local/bin/sessions"
TMUX_AGENTS_SESSION="${SESSIONS_TMUX_SESSION:-agents}"
TMUX_UI_SESSION="${SESSIONS_TMUX_UI_SESSION:-sessions-ui}"

issues=0

report_leftover() {
  printf '  [leftover] %s\n' "$1"
  issues=$((issues + 1))
}

report_ok() {
  printf '  [ok] %s\n' "$1"
}

echo "== verify uninstall =="

for path in \
  "${INSTALL_DIR}/sessions" \
  "$PATH_SESSIONS" \
  "${HOME_DIR}/.local/bin/sessionsd"; do
  if [[ -e "$path" || -L "$path" ]]; then
    report_leftover "$path"
  fi
done

if [[ -e "$SOCKET" || -L "$SOCKET" ]]; then
  report_leftover "$SOCKET"
fi
if [[ -d "$DATA_ROOT" ]]; then
  report_leftover "$DATA_ROOT/"
fi

if pgrep -f 'sessions daemon' >/dev/null 2>&1 || pgrep -f 'sessions bar' >/dev/null 2>&1; then
  report_leftover "running sessions daemon or sidebar process"
fi

if command -v tmux >/dev/null 2>&1; then
  for session in "$TMUX_UI_SESSION" "$TMUX_AGENTS_SESSION"; do
    if tmux has-session -t "$session" 2>/dev/null; then
      report_leftover "tmux session: $session"
    fi
  done
fi

if [[ "$issues" -eq 0 ]]; then
  report_ok "no sessions-cli install artifacts found"
  echo ""
  echo "Uninstall verified."
  exit 0
fi

echo ""
echo "Uninstall incomplete — $issues issue(s) above."
echo "Try: ./uninstall.sh --yes"
exit 1