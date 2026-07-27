#!/usr/bin/env bash
# Install sessions-cli from a repo checkout.
# One script: build, deploy, agent hooks, verify, start daemon.
# Run from repo root: ./install.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

NO_BUILD=false
SKIP_DEPS=false

usage() {
  cat <<EOF
Usage: ${ROOT}/install.sh [options]

Install sessions-cli for the current user. Configures detected agent
hooks (grok, codex, claude, opencode), verifies the install, and starts
the daemon. After install, open the sidebar with: sessions up

Options:
  --no-build    Deploy an existing target/release/sessions binary
  --skip-deps   Skip dependency checks (rust, tmux)
  -h, --help    Show this help
EOF
}

for arg in "$@"; do
  case "$arg" in
    --no-build) NO_BUILD=true ;;
    --skip-deps) SKIP_DEPS=true ;;
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

step_ok() {
  printf '  ok  %s\n' "$1"
}

step_skip() {
  printf '  —   %s\n' "$1"
}

step_fail() {
  printf '  FAIL %s\n' "$1" >&2
}

ensure_rust() {
  if command -v cargo >/dev/null 2>&1 && command -v rustc >/dev/null 2>&1; then
    step_ok "rust toolchain"
    return 0
  fi
  step_fail "rust toolchain not found"
  if command -v curl >/dev/null 2>&1; then
    echo "        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  else
    echo "        https://rustup.rs/" >&2
  fi
  return 1
}

ensure_tmux() {
  if command -v tmux >/dev/null 2>&1; then
    step_ok "tmux"
    return 0
  fi
  step_skip "tmux not installed"
  case "$(uname -s)" in
    Darwin) echo "        brew install tmux" >&2 ;;
    Linux)
      if command -v apt-get >/dev/null 2>&1; then
        echo "        sudo apt-get install -y tmux" >&2
      elif command -v dnf >/dev/null 2>&1; then
        echo "        sudo dnf install -y tmux" >&2
      else
        echo "        install tmux with your package manager" >&2
      fi
      ;;
    *) echo "        install tmux with your package manager" >&2 ;;
  esac
  return 0
}

warn_path_entry() {
  local home="$1"
  local path_dir="${home}/.local/bin"
  if [[ ":${PATH:-}:" != *":${path_dir}:"* ]]; then
    echo ""
    echo "Add ~/.local/bin to your PATH (e.g. in ~/.zshrc):"
    echo "  export PATH=\"\${HOME}/.local/bin:\${PATH}\""
    echo "Then open a new shell and run: sessions up"
    return 1
  fi
  return 0
}

install_agent_hooks() {
  local dest="$1"
  local output

  if ! output=$("$dest" hooks setup 2>&1); then
    step_fail "agent hooks"
    printf '%s\n' "$output" >&2
    return 1
  fi

  local reported=0
  while IFS= read -r line; do
    [[ "$line" == *" hooks:"* ]] || continue
    step_ok "$line"
    reported=1
  done < <("$dest" hooks status 2>/dev/null || true)

  if [[ "$reported" -eq 0 ]]; then
    if grep -q "no supported agents detected" <<<"$output"; then
      step_skip "no agent apps detected"
    else
      step_ok "agent hooks"
    fi
  fi
}

verify_install() {
  local dest="$1"
  if "$dest" doctor --quiet 2>/dev/null; then
    step_ok "install verification"
    return 0
  fi
  step_fail "install verification"
  "$dest" doctor >&2 || true
  return 1
}

start_daemon() {
  local dest="$1"
  "${ROOT}/bin/start-sessionsd.sh"
  if "$dest" status >/dev/null 2>&1; then
    step_ok "sessionsd"
    return 0
  fi
  step_fail "sessionsd failed to start"
  return 1
}

ensure_sessions_config() {
  local home="$1"
  local config_path
  config_path="$(sessions_config_path "$home")"
  local config_dir
  config_dir="$(dirname "$config_path")"
  mkdir -p "$config_dir"

  if [[ -f "$config_path" ]]; then
    step_ok "sessions config"
    return 0
  fi

  local install_id
  if command -v uuidgen >/dev/null 2>&1; then
    install_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
  else
    install_id="$(python3 -c 'import uuid; print(uuid.uuid4())')"
  fi

  cat >"$config_path" <<EOF
[telemetry]
level = "off"
install_id = "${install_id}"
channel = "stable"
install_method = "git"

[install]
checkout_path = "${ROOT}"

[update]
last_check_at = ""
available_version = ""
urgency = ""
message = ""
changelog_url = ""
EOF
  step_ok "sessions config (install_id created)"
}

echo "sessions-cli install"
echo "repo: ${ROOT}"

if [[ "$SKIP_DEPS" == false ]]; then
  echo ""
  echo "== dependencies =="
  ensure_rust
  ensure_tmux
fi

echo ""
echo "== build + deploy =="
if [[ "$NO_BUILD" == true ]]; then
  "${ROOT}/bin/dev-install.sh" --no-build
else
  "${ROOT}/bin/dev-install.sh"
fi

HOME_DIR="$(sessions_home)"
DEST="$(sessions_install_dir "$HOME_DIR")/sessions"

if [[ ! -x "${DEST}" ]]; then
  step_fail "sessions binary missing after deploy"
  exit 1
fi

echo ""
echo "== sessions config =="
ensure_sessions_config "$HOME_DIR"

echo ""
echo "== companion scripts (optional MCP / Skills) =="
# Deploy ensure scripts only — do not install Docker/Obot/skillshare at install time.
# Users set those up from the MCP / Skills panels when they need them.
SCRIPTS_DIR="$(sessions_scripts_dir "$HOME_DIR")"
mkdir -p "$SCRIPTS_DIR"
install -m 755 "${ROOT}/bin/ensure-skillshare.sh" "${SCRIPTS_DIR}/ensure-skillshare.sh"
install -m 755 "${ROOT}/bin/ensure-obot.sh" "${SCRIPTS_DIR}/ensure-obot.sh"
install -m 755 "${ROOT}/bin/ensure-companions.sh" "${SCRIPTS_DIR}/ensure-companions.sh"
install -m 644 "${ROOT}/bin/paths.sh" "${SCRIPTS_DIR}/paths.sh"
step_ok "companion scripts deployed (setup from MCP / Skills panels)"

echo ""
echo "== agent hooks =="
install_agent_hooks "${DEST}"

if command -v ghostty >/dev/null 2>&1; then
  echo ""
  echo "== ghostty keybinds =="
  "${ROOT}/bin/setup-ghostty.sh"
fi

# Cursor steals ⌘1–⌘9 (editor groups) / ⌘0 (zoom); route to terminal for sessions.
# Also sets terminal.integrated.minimumContrastRatio=1 so truecolor isn't washed out,
# rightClickBehavior=nothing so IDE menus don't steal sidebar right-clicks, hybrid
# right-click JS (workspace → IDE menu), and macOS Press-and-Hold off for hold-d.
if [[ -d "${HOME_DIR}/Library/Application Support/Cursor" ]] \
  || [[ -d "${HOME_DIR}/.config/Cursor" ]] \
  || command -v cursor >/dev/null 2>&1; then
  echo ""
  echo "== cursor keybinds + terminal color =="
  "${ROOT}/bin/setup-cursor.sh"
fi

# VS Code integrated terminal: same contrast/backdrop/right-click fidelity as Cursor.
if [[ -d "${HOME_DIR}/Library/Application Support/Code" ]] \
  || [[ -d "${HOME_DIR}/.config/Code" ]] \
  || command -v code >/dev/null 2>&1; then
  echo ""
  echo "== vscode terminal color =="
  "${ROOT}/bin/setup-vscode.sh"
fi

echo ""
echo "== verify =="
verify_install "${DEST}"

echo ""
echo "== daemon =="
start_daemon "${DEST}"

PATH_OK=true
warn_path_entry "$HOME_DIR" || PATH_OK=false

echo ""
echo "Install complete."
echo ""
echo "Telemetry is off by default. See docs/telemetry.md to opt in."
echo ""

if [[ "$PATH_OK" == true ]] && command -v tmux >/dev/null 2>&1; then
  echo "Open the sidebar:"
  echo "  sessions up"
elif [[ "$PATH_OK" == true ]]; then
  echo "Install tmux, then open the sidebar:"
  echo "  sessions up"
fi