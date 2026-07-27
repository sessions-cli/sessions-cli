#!/usr/bin/env bash
# Install a release-built sessions binary to the canonical deploy path.
# Handles macOS signing and PATH symlinks.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

SRC_BINARY="${1:?usage: install-binary.sh <path-to-release-binary>}"

if [[ ! -f "$SRC_BINARY" ]]; then
  echo "sessions binary not found: ${SRC_BINARY}" >&2
  exit 1
fi

HOME_DIR="$(sessions_home)"
DEST="$(sessions_install_dir "$HOME_DIR")/sessions"
SCRIPTS_DIR="$(sessions_scripts_dir "$HOME_DIR")"
STATE_DIR="$(sessions_state_dir "$HOME_DIR")"
LOGS_DIR="$(sessions_logs_dir "$HOME_DIR")"

mkdir -p "$(dirname "$DEST")" "${HOME_DIR}/.local/bin" "$SCRIPTS_DIR" "$STATE_DIR" "$LOGS_DIR"

# Always install a real file at the canonical path (never a symlink).
rm -f "$DEST"
install -m 755 "$SRC_BINARY" "$DEST"

if ! cmp -s "$SRC_BINARY" "$DEST"; then
  echo "installed binary does not match source build" >&2
  exit 1
fi

if [[ "$(uname -s)" == Darwin ]]; then
  SIGN_IDENTITY="${SESSIONS_CODESIGN_IDENTITY:-}"
  if [[ -z "$SIGN_IDENTITY" ]]; then
    SIGN_IDENTITY="$(
      security find-identity -v -p codesigning 2>/dev/null \
        | awk -F\" '/^[[:space:]]*[0-9]+\)/ { print $2; exit }'
    )"
  fi
  if [[ -n "$SIGN_IDENTITY" ]]; then
    echo "Signing with identity: ${SIGN_IDENTITY}"
    codesign -s "$SIGN_IDENTITY" --force "$DEST"
  else
    echo "Signing ad-hoc: no local code-signing identity found"
    codesign -s - --force "$DEST"
  fi
  codesign --verify --verbose "$DEST" >/dev/null
fi

if ! "$DEST" --help >/dev/null 2>&1; then
  echo "installed binary failed to execute (check code signature)" >&2
  exit 1
fi

# PATH entries are symlinks only — never copy binaries here directly.
rm -f "${HOME_DIR}/.local/bin/sessions" "${HOME_DIR}/.local/bin/sessionsd" \
  "${SCRIPTS_DIR}/sessionsd"
ln -sf "$DEST" "${SCRIPTS_DIR}/sessionsd"
ln -sf "$DEST" "${HOME_DIR}/.local/bin/sessions"
ln -sf "$DEST" "${HOME_DIR}/.local/bin/sessionsd"

# Grok Ctrl+N and legacy ~/.grok/scripts helpers expect this path.
if [[ -d "${HOME_DIR}/.grok" || -n "${GROK_HOME:-}" ]]; then
  GROK_LEGACY="$(grok_legacy_sessions_binary "$HOME_DIR")"
  mkdir -p "$(dirname "$GROK_LEGACY")"
  rm -f "$GROK_LEGACY"
  ln -sf "$DEST" "$GROK_LEGACY"
fi

if [[ -L "$DEST" ]]; then
  echo "canonical sessions path must be a regular file, not a symlink" >&2
  exit 1
fi