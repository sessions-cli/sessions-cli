#!/usr/bin/env bash
# Shared path resolution for sessions install scripts.
# Source this file: . "$(dirname "$0")/paths.sh"
set -euo pipefail

sessions_home() {
  if [[ -n "${HOME:-}" ]]; then
    printf '%s' "$HOME"
    return
  fi
  if command -v getent >/dev/null 2>&1; then
    getent passwd "$(id -un)" | cut -d: -f6
    return
  fi
  printf '%s' "$(eval printf '~%s' "$(id -un)")"
}

sessions_xdg_data_home() {
  local home="$1"
  if [[ -n "${XDG_DATA_HOME:-}" ]]; then
    printf '%s' "$XDG_DATA_HOME"
  else
    printf '%s/.local/share' "$home"
  fi
}

sessions_data_root() {
  local home="$1"
  if [[ -n "${SESSIONS_DATA_DIR:-}" ]]; then
    printf '%s' "$SESSIONS_DATA_DIR"
    return
  fi
  printf '%s/sessions' "$(sessions_xdg_data_home "$home")"
}

sessions_install_dir() {
  local home="$1"
  if [[ -n "${SESSIONS_INSTALL_DIR:-}" ]]; then
    printf '%s' "$SESSIONS_INSTALL_DIR"
    return
  fi
  printf '%s/sessions/bin' "$(sessions_xdg_data_home "$home")"
}

sessions_binary() {
  local home="$1"
  if [[ -n "${SESSIONS_BIN:-}" && -f "$SESSIONS_BIN" ]]; then
    printf '%s' "$SESSIONS_BIN"
    return
  fi
  if [[ -f "$home/.local/bin/sessions" ]]; then
    printf '%s/.local/bin/sessions' "$home"
    return
  fi
  local installed
  installed="$(sessions_install_dir "$home")/sessions"
  if [[ -f "$installed" ]]; then
    printf '%s' "$installed"
    return
  fi
  printf '%s' "sessions"
}

sessions_state_dir() {
  local home="$1"
  printf '%s/state' "$(sessions_data_root "$home")"
}

sessions_logs_dir() {
  local home="$1"
  printf '%s/logs' "$(sessions_data_root "$home")"
}

sessions_scripts_dir() {
  local home="$1"
  printf '%s/scripts' "$(sessions_data_root "$home")"
}

grok_scripts_dir() {
  local home="$1"
  if [[ -n "${GROK_HOME:-}" ]]; then
    printf '%s/scripts' "$GROK_HOME"
    return
  fi
  printf '%s/.grok/scripts' "$home"
}

grok_legacy_sessions_binary() {
  local home="$1"
  printf '%s/sessions' "$(grok_scripts_dir "$home")"
}

sessions_config_dir() {
  local home="$1"
  if [[ -n "${XDG_CONFIG_HOME:-}" ]]; then
    printf '%s/sessions' "$XDG_CONFIG_HOME"
    return
  fi
  printf '%s/.config/sessions' "$home"
}

sessions_config_path() {
  local home="$1"
  printf '%s/config.toml' "$(sessions_config_dir "$home")"
}