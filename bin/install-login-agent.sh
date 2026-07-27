#!/usr/bin/env bash
# Install a macOS LaunchAgent that starts sessions companions at login.
# No-op on non-Darwin. Soft-fails if launchctl unavailable.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"

if [[ "$(uname -s)" != "Darwin" ]]; then
  exit 0
fi

HOME_DIR="$(sessions_home)"
SCRIPTS_DIR="$(sessions_scripts_dir "$HOME_DIR")"
LABEL="ai.sessions.companions"
PLIST_DIR="${HOME_DIR}/Library/LaunchAgents"
PLIST="${PLIST_DIR}/${LABEL}.plist"
ENSURE="${SCRIPTS_DIR}/ensure-companions.sh"
LOG_DIR="$(sessions_logs_dir "$HOME_DIR")"
mkdir -p "$PLIST_DIR" "$LOG_DIR" "$SCRIPTS_DIR"

if [[ ! -x "$ENSURE" ]]; then
  # Fall back to repo script during install before deploy finishes.
  if [[ -x "${ROOT}/bin/ensure-companions.sh" ]]; then
    install -m 755 "${ROOT}/bin/ensure-companions.sh" "$ENSURE"
    install -m 755 "${ROOT}/bin/ensure-skillshare.sh" "${SCRIPTS_DIR}/ensure-skillshare.sh"
    install -m 755 "${ROOT}/bin/ensure-obot.sh" "${SCRIPTS_DIR}/ensure-obot.sh"
    install -m 644 "${ROOT}/bin/paths.sh" "${SCRIPTS_DIR}/paths.sh"
  else
    echo "  —   login agent: ensure-companions.sh not deployed yet" >&2
    exit 0
  fi
fi

cat >"$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>${LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/bash</string>
    <string>${ENSURE}</string>
    <string>--quiet</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StartInterval</key>
  <integer>3600</integer>
  <key>StandardOutPath</key>
  <string>${LOG_DIR}/companions.log</string>
  <key>StandardErrorPath</key>
  <string>${LOG_DIR}/companions.log</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>${HOME_DIR}</string>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${HOME_DIR}/.local/bin</string>
  </dict>
</dict>
</plist>
EOF

if command -v launchctl >/dev/null 2>&1; then
  uid="$(id -u)"
  launchctl bootout "gui/${uid}/${LABEL}" 2>/dev/null || true
  if launchctl bootstrap "gui/${uid}" "$PLIST" 2>/dev/null \
    || launchctl load -w "$PLIST" 2>/dev/null; then
    echo "  ok  login agent: ${LABEL}"
  else
    echo "  —   login agent: plist written (${PLIST}); load manually if needed"
  fi
else
  echo "  ok  login agent: plist written (${PLIST})"
fi
