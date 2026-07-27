#!/usr/bin/env bash
# Merge Ghostty keybind overrides so ⌘ keys reach sessions (tmux Meta / ESC prefixes).
# Ghostty defaults steal super+n (new_window), super+, (open_config), and often
# super+1..9 (tabs). Route them as Meta so tmux / the sidebar can handle them.
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
# Route ⌘ shortcuts to tmux Meta sequences (Ghostty defaults steal several of these).
keybind = super+n=text:\x1bn
keybind = super+shift+n=text:\x1bN
keybind = super+,=text:\x1b,
keybind = super+f=text:\x1bf
keybind = super+b=text:\x1bb
# Sidebar ordinal focus: ⌘1–⌘9, ⌘0 → sessions focus 1–10 (tmux M-1..M-0).
keybind = super+1=text:\x1b1
keybind = super+2=text:\x1b2
keybind = super+3=text:\x1b3
keybind = super+4=text:\x1b4
keybind = super+5=text:\x1b5
keybind = super+6=text:\x1b6
keybind = super+7=text:\x1b7
keybind = super+8=text:\x1b8
keybind = super+9=text:\x1b9
keybind = super+0=text:\x1b0
# Sidebar width: ⌘⇧[ / ⌘⇧] → Meta+{ / Meta+} (tmux → sessions resize-sidebar).
# Plain [ / ] also work when the sidebar list has focus.
keybind = super+shift+[=text:\x1b{
keybind = super+shift+]=text:\x1b}
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
