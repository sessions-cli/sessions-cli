#!/usr/bin/env bash
# Merge Ghostty keybind overrides so ⌘+N / ⌘+, reach sessions (tmux M-n / M-Comma).
# Ghostty defaults bind super+n=new_window and super+,=open_config.
set -euo pipefail

MARKER_START="# >>> sessions-cli ghostty keybinds >>>"
MARKER_END="# <<< sessions-cli ghostty keybinds <<<"

ghostty_config_path() {
  if [[ "$(uname -s)" == Darwin ]]; then
    printf '%s\n' "${HOME}/Library/Application Support/com.mitchellh.ghostty/config"
  else
    printf '%s\n' "${HOME}/.config/ghostty/config"
  fi
}

read -r -d '' BLOCK <<EOF || true
${MARKER_START}
# Route ⌘+N and ⌘+, to tmux Meta sequences (Ghostty defaults steal these).
keybind = super+n=text:\x1bn
keybind = super+shift+n=text:\x1bN
keybind = super+,=text:\x1b,
${MARKER_END}
EOF

CONFIG="$(ghostty_config_path)"
mkdir -p "$(dirname "$CONFIG")"
touch "$CONFIG"

TMP="$(mktemp)"
awk -v start="$MARKER_START" -v end="$MARKER_END" '
  $0 == start { skip = 1; next }
  $0 == end { skip = 0; next }
  !skip { print }
' "$CONFIG" >"$TMP"
printf '\n%s\n' "$BLOCK" >>"$TMP"
mv "$TMP" "$CONFIG"

echo "ghostty: merged sessions keybinds into ${CONFIG}"