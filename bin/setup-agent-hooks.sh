#!/usr/bin/env bash
# Install sessions notify hooks for grok, codex, claude, or opencode.
# Usage: setup-agent-hooks.sh [grok|codex|claude|opencode]
# Omit agent to configure all detected agents.
set -euo pipefail

AGENT="${1:-}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"
HOME_DIR="$(sessions_home)"
SESSIONS_BIN="$(sessions_binary "$HOME_DIR")"

if [[ ! -x "$SESSIONS_BIN" ]]; then
  echo "sessions binary not found: $SESSIONS_BIN" >&2
  echo "Install first: ${ROOT}/install.sh" >&2
  exit 1
fi

if [[ -z "$AGENT" ]]; then
  exec "$SESSIONS_BIN" hooks setup
fi

case "$AGENT" in
  grok|codex|claude|opencode)
    exec "$SESSIONS_BIN" hooks setup "$AGENT"
    ;;
  *)
    echo "Usage: setup-agent-hooks.sh [grok|codex|claude|opencode]" >&2
    exit 1
    ;;
esac