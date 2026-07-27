#!/usr/bin/env bash
# Ensure skillshare CLI is installed and initialized for sessions Skills panel.
# Portable: brew preferred, curl install script fallback. Soft-fails with hints.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "${ROOT}/bin/paths.sh" ]]; then
  # shellcheck source=paths.sh
  . "${ROOT}/bin/paths.sh"
elif [[ -f "$(dirname "$0")/paths.sh" ]]; then
  # shellcheck source=paths.sh
  . "$(dirname "$0")/paths.sh"
else
  sessions_home() { printf '%s' "${HOME:-}"; }
fi

HOME_DIR="$(sessions_home)"
QUIET=false
for arg in "$@"; do
  case "$arg" in
    --quiet|-q) QUIET=true ;;
  esac
done

log() {
  if [[ "$QUIET" == false ]]; then
    printf '%s\n' "$*"
  fi
}

step_ok() { log "  ok  skillshare: $1"; }
step_skip() { log "  —   skillshare: $1"; }
step_fail() { log "  FAIL skillshare: $1" >&2; }

find_skillshare() {
  if [[ -n "${SKILLSHARE_BIN:-}" && -x "${SKILLSHARE_BIN}" ]]; then
    printf '%s' "${SKILLSHARE_BIN}"
    return 0
  fi
  if command -v skillshare >/dev/null 2>&1; then
    command -v skillshare
    return 0
  fi
  for candidate in \
    "${HOME_DIR}/.local/bin/skillshare" \
    /opt/homebrew/bin/skillshare \
    /usr/local/bin/skillshare \
    "${HOME_DIR}/go/bin/skillshare"; do
    if [[ -x "$candidate" ]]; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  return 1
}

install_skillshare() {
  if command -v brew >/dev/null 2>&1; then
    log "  …   installing skillshare via Homebrew"
    if brew install skillshare; then
      return 0
    fi
    step_fail "brew install skillshare failed; trying curl installer"
  fi
  if command -v curl >/dev/null 2>&1; then
    log "  …   installing skillshare via curl installer"
    if curl -fsSL https://raw.githubusercontent.com/runkids/skillshare/main/install.sh | sh; then
      return 0
    fi
  fi
  return 1
}

init_skillshare() {
  local bin="$1"
  local store="${XDG_CONFIG_HOME:-${HOME_DIR}/.config}/skillshare"
  if [[ -d "${store}/skills" ]] || [[ -f "${store}/config.toml" ]] || [[ -f "${store}/config.yaml" ]]; then
    step_ok "store ready (${store})"
    return 0
  fi
  log "  …   skillshare init"
  if "$bin" init >/dev/null 2>&1; then
    step_ok "initialized"
    return 0
  fi
  # Non-interactive re-init may fail if partially present; still ok if binary works.
  step_skip "init skipped or already configured"
  return 0
}

main() {
  local bin
  if bin="$(find_skillshare)"; then
    step_ok "found ($bin)"
  else
    if install_skillshare; then
      if bin="$(find_skillshare)"; then
        step_ok "installed ($bin)"
      else
        step_fail "installed but binary not on PATH — open a new shell or add ~/.local/bin"
        return 1
      fi
    else
      step_fail "not installed — brew install skillshare  OR  curl -fsSL https://raw.githubusercontent.com/runkids/skillshare/main/install.sh | sh"
      return 1
    fi
  fi
  init_skillshare "$bin"
  return 0
}

main "$@"
