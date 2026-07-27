#!/usr/bin/env bash
# Remove sessions-cli binaries, data, and tmux sessions.
# Run from repo root: ./uninstall.sh
# Or: curl -fsSL https://raw.githubusercontent.com/sessions-cli/sessions-cli/main/uninstall.sh | bash
set -euo pipefail

INSTALL_URL="${SESSIONS_INSTALL_URL:-https://raw.githubusercontent.com/sessions-cli/sessions-cli/main/install.sh}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

YES=false
DRY_RUN=false
PURGE_CONFIG=false
KEEP_CONFIG=false

TMUX_AGENTS_SESSION="${SESSIONS_TMUX_SESSION:-agents}"
TMUX_UI_SESSION="${SESSIONS_TMUX_UI_SESSION:-sessions-ui}"

usage() {
  cat <<EOF
Usage: ${ROOT}/uninstall.sh [options]
       ${ROOT}/bin/uninstall.sh [options]

Remove sessions-cli binaries, data, and tmux sessions.

Interactive by default: you choose whether to also remove workspace
configuration (~/.config/sessions, including workspaces.toml).

Options:
  --yes, -y         Non-interactive: proceed without prompts (keeps config
                    unless --purge-config is also set)
  --purge-config    Non-interactive: also remove ~/.config/sessions
  --keep-config     Non-interactive: keep ~/.config/sessions
  --dry-run         Show what would be removed without changing anything
  -h, --help        Show this help

Reinstall:
  curl -fsSL ${INSTALL_URL} | bash
  sessions up
EOF
}

for arg in "$@"; do
  case "$arg" in
    --yes|-y) YES=true ;;
    --purge-config) PURGE_CONFIG=true ;;
    --keep-config) KEEP_CONFIG=true ;;
    --dry-run) DRY_RUN=true ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$PURGE_CONFIG" == true && "$KEEP_CONFIG" == true ]]; then
  echo "cannot use --purge-config and --keep-config together" >&2
  exit 1
fi

HOME_DIR="$(sessions_home)"
INSTALL_DIR="$(sessions_install_dir "$HOME_DIR")"
DATA_ROOT="$(sessions_data_root "$HOME_DIR")"
SCRIPTS_DIR="$(sessions_scripts_dir "$HOME_DIR")"
STATE_DIR="$(sessions_state_dir "$HOME_DIR")"
LOGS_DIR="$(sessions_logs_dir "$HOME_DIR")"
CONFIG_DIR="$(sessions_config_dir "$HOME_DIR")"
SOCKET="${HOME_DIR}/.local/run/sessionsd.sock"
SPOOL_DIR="${DATA_ROOT}/spool"

PATH_SESSIONS="${HOME_DIR}/.local/bin/sessions"
PATH_SESSIONSD="${HOME_DIR}/.local/bin/sessionsd"

CANONICAL_BINARY="${INSTALL_DIR}/sessions"
CANONICAL_SESSIONSD="${SCRIPTS_DIR}/sessionsd"

declare -a TARGETS=()

add_target() {
  local path="$1"
  if [[ -e "$path" || -L "$path" ]]; then
    TARGETS+=("$path")
  fi
}

unload_login_agent() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    return 0
  fi
  local label="ai.sessions.companions"
  local plist="${HOME_DIR}/Library/LaunchAgents/${label}.plist"
  if command -v launchctl >/dev/null 2>&1; then
    local uid
    uid="$(id -u)"
    if [[ "$DRY_RUN" == true ]]; then
      printf '  would unload: launchctl bootout gui/%s/%s\n' "$uid" "$label"
    else
      launchctl bootout "gui/${uid}/${label}" 2>/dev/null || true
      launchctl unload -w "$plist" 2>/dev/null || true
    fi
  fi
}

collect_targets() {
  TARGETS=()

  add_target "$PATH_SESSIONS"
  add_target "$PATH_SESSIONSD"
  add_target "$CANONICAL_BINARY"
  add_target "$CANONICAL_SESSIONSD"
  add_target "${SCRIPTS_DIR}/ensure-sidebar.sh"
  add_target "${SCRIPTS_DIR}/ensure-skillshare.sh"
  add_target "${SCRIPTS_DIR}/ensure-obot.sh"
  add_target "${SCRIPTS_DIR}/ensure-companions.sh"
  add_target "${HOME_DIR}/Library/LaunchAgents/ai.sessions.companions.plist"
  add_target "$SOCKET"
  add_target "$STATE_DIR"
  add_target "$SPOOL_DIR"
  add_target "$LOGS_DIR"
  add_target "$SCRIPTS_DIR"
  add_target "$INSTALL_DIR"
  add_target "$DATA_ROOT"

  if [[ "$PURGE_CONFIG" == true ]]; then
    add_target "$CONFIG_DIR"
  fi
}

run_cmd() {
  if [[ "$DRY_RUN" == true ]]; then
    printf '  would run: %s\n' "$*"
    return 0
  fi
  "$@"
}

remove_path() {
  local path="$1"
  if [[ "$DRY_RUN" == true ]]; then
    if [[ -d "$path" && ! -L "$path" ]]; then
      printf '  would remove dir:  %s\n' "$path"
    else
      printf '  would remove:      %s\n' "$path"
    fi
    return 0
  fi
  if [[ -d "$path" && ! -L "$path" ]]; then
    rm -rf "$path"
    printf '  removed dir:  %s\n' "$path"
  elif [[ -e "$path" || -L "$path" ]]; then
    rm -f "$path"
    printf '  removed:      %s\n' "$path"
  fi
}

stop_processes() {
  echo "== stop processes =="
  local sessions_bin=""
  if [[ -x "$CANONICAL_BINARY" ]]; then
    sessions_bin="$CANONICAL_BINARY"
  elif [[ -x "$PATH_SESSIONS" ]]; then
    sessions_bin="$PATH_SESSIONS"
  elif command -v sessions >/dev/null 2>&1; then
    sessions_bin="$(command -v sessions)"
  fi

  if [[ -n "$sessions_bin" ]]; then
    if [[ "$DRY_RUN" == true ]]; then
      printf '  would run: %s down\n' "$sessions_bin"
    else
      "$sessions_bin" down >/dev/null 2>&1 || true
      printf '  ran: %s down\n' "$sessions_bin"
    fi
  fi

  local pattern
  for pattern in \
    "${CANONICAL_BINARY} daemon" \
    "${PATH_SESSIONS} daemon" \
    "${CANONICAL_BINARY} bar" \
    "${PATH_SESSIONS} bar" \
    "sessions daemon" \
    "sessions bar"; do
    if [[ "$DRY_RUN" == true ]]; then
      if pgrep -f "$pattern" >/dev/null 2>&1; then
        printf '  would stop processes matching: %s\n' "$pattern"
      fi
    elif pgrep -f "$pattern" >/dev/null 2>&1; then
      pkill -f "$pattern" 2>/dev/null || true
      printf '  stopped processes matching: %s\n' "$pattern"
    fi
  done
  sleep 0.2
  run_cmd rm -f "$SOCKET"
}

kill_tmux_sessions() {
  if ! command -v tmux >/dev/null 2>&1; then
    return 0
  fi

  echo ""
  echo "== kill tmux sessions =="
  local session
  for session in "$TMUX_UI_SESSION" "$TMUX_AGENTS_SESSION"; do
    if tmux has-session -t "$session" 2>/dev/null; then
      if [[ "$DRY_RUN" == true ]]; then
        printf '  would kill tmux session: %s\n' "$session"
      else
        tmux kill-session -t "$session"
        printf '  killed tmux session: %s\n' "$session"
      fi
    fi
  done
}

interactive_tty() {
  [[ -t 0 || ( -e /dev/tty && -r /dev/tty ) ]]
}

prompt_reply() {
  local prompt="$1"
  local reply=""
  if [[ -t 0 ]]; then
    read -r -p "$prompt" reply
  elif [[ -r /dev/tty ]]; then
    read -r -p "$prompt" reply </dev/tty
  else
    return 1
  fi
  printf '%s' "$reply"
}

confirm_purge_config() {
  if [[ "$PURGE_CONFIG" == true || "$KEEP_CONFIG" == true ]]; then
    return 0
  fi
  if [[ "$YES" == true ]]; then
    return 0
  fi
  if [[ ! -d "$CONFIG_DIR" ]]; then
    return 0
  fi
  if ! interactive_tty; then
    echo "no interactive terminal — pass --purge-config or --keep-config" >&2
    exit 1
  fi

  echo ""
  echo "Uninstall intensity:"
  echo "  Standard — remove binaries and runtime data; keep project list"
  echo "  Full     — also remove ${CONFIG_DIR}/ (workspaces.toml, etc.)"
  echo ""
  local reply
  reply="$(prompt_reply "Also remove workspace configuration? [y/N] ")" || {
    echo "aborted" >&2
    exit 1
  }
  case "$reply" in
    y|Y|yes|YES) PURGE_CONFIG=true ;;
    *) ;;
  esac
}

confirm_removal() {
  if [[ "$YES" == true || "$DRY_RUN" == true ]]; then
    return 0
  fi
  if ! interactive_tty; then
    echo "no interactive terminal — pass --yes to proceed non-interactively" >&2
    exit 1
  fi

  echo ""
  echo "The following will be removed:"
  for path in "${TARGETS[@]}"; do
    echo "  $path"
  done
  if [[ "$PURGE_CONFIG" == false && -d "$CONFIG_DIR" ]]; then
    echo ""
    echo "Workspace configuration will be kept:"
    echo "  ${CONFIG_DIR}/"
  fi
  echo ""
  echo "tmux sessions to kill: ${TMUX_UI_SESSION}, ${TMUX_AGENTS_SESSION}"
  echo ""
  local reply
  reply="$(prompt_reply "Continue? [y/N] ")" || {
    echo "aborted" >&2
    exit 1
  }
  case "$reply" in
    y|Y|yes|YES) ;;
    *)
      echo "aborted"
      exit 1
      ;;
  esac
}

echo "sessions-cli uninstall"
echo "home: ${HOME_DIR}"

stop_processes
kill_tmux_sessions
collect_targets
confirm_purge_config
collect_targets

if [[ ${#TARGETS[@]} -eq 0 ]]; then
  echo ""
  echo "Nothing to remove."
  exit 0
fi

confirm_removal

echo ""
echo "== remove install artifacts =="
unload_login_agent
for path in "${TARGETS[@]}"; do
  remove_path "$path"
done

echo ""
if [[ "$DRY_RUN" == true ]]; then
  echo "Dry run complete."
else
  if [[ "$PURGE_CONFIG" == true ]]; then
    echo "Uninstall complete (workspace configuration removed)."
  else
    echo "Uninstall complete (workspace configuration kept at ${CONFIG_DIR}/)."
  fi

  echo ""
  if "${ROOT}/bin/verify-uninstall.sh"; then
    echo ""
    echo "Reinstall:"
    echo "  curl -fsSL ${INSTALL_URL} | bash"
  fi
fi