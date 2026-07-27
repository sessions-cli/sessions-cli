#!/usr/bin/env bash
# Verify tmux UI session mouse bindings and layout after reload/up.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
if [[ -f "${SCRIPT_DIR}/paths.sh" ]]; then
  # shellcheck source=paths.sh
  . "${SCRIPT_DIR}/paths.sh"
  HOME_DIR="$(sessions_home)"
  SESSIONS="$(sessions_binary "$HOME_DIR")"
else
  HOME_DIR="${HOME:?}"
  SESSIONS="$(command -v sessions)"
fi

UI_SESSION="${SESSIONS_UI_SESSION:-sessions-ui}"
AGENTS_SESSION="${SESSIONS_AGENTS_SESSION:-agents}"

if ! command -v tmux >/dev/null 2>&1; then
  echo "verify-ui-runtime: tmux not installed" >&2
  exit 1
fi

if ! tmux has-session -t "$UI_SESSION" 2>/dev/null; then
  echo "verify-ui-runtime: $UI_SESSION missing — run: sessions up" >&2
  exit 1
fi

if ! tmux show-options -t "$UI_SESSION" mouse 2>/dev/null | grep -q 'mouse on'; then
  echo "verify-ui-runtime: $UI_SESSION mouse is not on" >&2
  exit 1
fi

binding="$(tmux list-keys -T root MouseDown1Pane 2>/dev/null | head -1 || true)"
if [[ -z "$binding" ]] || [[ "$binding" != *"$UI_SESSION"* ]]; then
  echo "verify-ui-runtime: MouseDown1Pane binding missing $UI_SESSION guard" >&2
  echo "  got: ${binding:-<empty>}" >&2
  exit 1
fi
if [[ "$binding" == *'\\;'* ]] || [[ "$binding" == *"select-pane -t = \\;"* ]]; then
  echo "verify-ui-runtime: MouseDown1Pane uses broken quoted \\; chain (re-run make reload)" >&2
  exit 1
fi
if [[ "$binding" == *"{ select-pane"* ]] || [[ "$binding" == *"{ send-keys"* ]]; then
  echo "verify-ui-runtime: MouseDown1Pane uses brace groups that cause runtime syntax errors (re-run make reload)" >&2
  exit 1
fi
if [[ "$binding" != *"select-pane -t = ; send-keys -M"* ]]; then
  echo "verify-ui-runtime: MouseDown1Pane missing select-pane + send-keys -M command chain (re-run make reload)" >&2
  exit 1
fi

for multi_key in DoubleClick1Pane TripleClick1Pane; do
  multi_binding="$(tmux list-keys -T root "$multi_key" 2>/dev/null | head -1 || true)"
  if [[ -z "$multi_binding" ]] || [[ "$multi_binding" != *"$UI_SESSION"* ]]; then
    echo "verify-ui-runtime: $multi_key binding missing $UI_SESSION guard" >&2
    echo "  got: ${multi_binding:-<empty>}" >&2
    exit 1
  fi
  if [[ "$multi_binding" == *"copy-mode"* ]]; then
    echo "verify-ui-runtime: $multi_key must not use copy-mode (re-run make reload)" >&2
    exit 1
  fi
  if [[ "$multi_binding" == *"{ select-pane"* ]] || [[ "$multi_binding" == *"{ send-keys"* ]]; then
    echo "verify-ui-runtime: $multi_key uses brace groups that cause runtime syntax errors (re-run make reload)" >&2
    exit 1
  fi
done




if ! pgrep -f "${SESSIONS} bar" >/dev/null 2>&1; then
  echo "verify-ui-runtime: sessions bar process not running" >&2
  exit 1
fi

# Nested workspace attach (sessions-ui:ui.1 → tmux attach -t agents) is intentional.
# Only flag host-level bare attaches whose TTY is not the workspace pane TTY.
WORKSPACE_TTY="$(tmux display-message -p -t "${UI_SESSION}:ui.1" '#{pane_tty}' 2>/dev/null || true)"
STRAYS=()
while IFS=$'\t' read -r tty session; do
  [[ -z "$tty" || -z "$session" ]] && continue
  if [[ "$session" != "$AGENTS_SESSION" ]]; then
    continue
  fi
  if [[ -n "$WORKSPACE_TTY" && "$tty" == "$WORKSPACE_TTY" ]]; then
    continue
  fi
  STRAYS+=("$tty")
done < <(tmux list-clients -F '#{client_tty}\t#{client_session}' 2>/dev/null || true)

if ((${#STRAYS[@]} > 0)); then
  echo "verify-ui-runtime: detaching bare client(s) on $AGENTS_SESSION (not nested workspace): ${STRAYS[*]}" >&2
  # Never detach WORKSPACE_TTY — that is the intentional nested agents attach.
  for tty in "${STRAYS[@]}"; do
    tmux detach-client -t "$tty" 2>/dev/null || true
  done
fi

echo "verify-ui-runtime: ok ($UI_SESSION mouse + bindings + bar process)"