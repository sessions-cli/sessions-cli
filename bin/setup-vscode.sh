#!/usr/bin/env bash
# Merge VS Code (Code) user settings so the integrated terminal shows sessions
# truecolor faithfully — not the default "reduced" palette from contrast rewriting.
#
# Root cause: terminal.integrated.minimumContrastRatio defaults to 4.5 and
# rewrites cell colors until WCAG contrast is met, desaturating truecolor.
# Sessions is RGB-heavy (sidebar greys, done-green, warm accents).
#
# Portable: only touches Code User settings when a Code install footprint exists.
# Safe to re-run (idempotent key merges).
set -euo pipefail

vscode_user_dir() {
  if [[ "$(uname -s)" == Darwin ]]; then
    printf '%s\n' "${HOME}/Library/Application Support/Code/User"
  else
    printf '%s\n' "${HOME}/.config/Code/User"
  fi
}

USER_DIR="$(vscode_user_dir)"
# Parent "Code" dir is created by first launch; skip if neither exists.
if [[ ! -d "$(dirname "$USER_DIR")" ]] && [[ ! -d "$USER_DIR" ]]; then
  echo "vscode: no Code user dir ($(dirname "$USER_DIR")) — skip"
  exit 0
fi

mkdir -p "$USER_DIR"
SETTINGS="${USER_DIR}/settings.json"
if [[ ! -f "$SETTINGS" ]]; then
  printf '%s\n' '{' '}' >"$SETTINGS"
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export SESSIONS_VSCODE_SETTINGS="$SETTINGS"
export SESSIONS_VSCODE_CSS_SRC="${ROOT}/integrations/cursor/sessions-terminal.css"
export SESSIONS_VSCODE_JS_SRC="${ROOT}/integrations/cursor/sessions-terminal.js"
export SESSIONS_DATA_DIR="${SESSIONS_DATA_DIR:-}"

python3 - <<'PY'
import json
import os
import re
import shutil
import sys

settings_path = os.environ["SESSIONS_VSCODE_SETTINGS"]


def strip_jsonc(text: str) -> str:
    out = []
    i = 0
    n = len(text)
    in_str = False
    esc = False
    while i < n:
        ch = text[i]
        if in_str:
            out.append(ch)
            if esc:
                esc = False
            elif ch == "\\":
                esc = True
            elif ch == '"':
                in_str = False
            i += 1
            continue
        if ch == '"':
            in_str = True
            out.append(ch)
            i += 1
            continue
        if ch == "/" and i + 1 < n and text[i + 1] == "/":
            i += 2
            while i < n and text[i] not in "\r\n":
                i += 1
            continue
        if ch == "/" and i + 1 < n and text[i + 1] == "*":
            i += 2
            while i + 1 < n and not (text[i] == "*" and text[i + 1] == "/"):
                i += 1
            i = min(n, i + 2)
            continue
        out.append(ch)
        i += 1
    return "".join(out)


def strip_trailing_commas(text: str) -> str:
    return re.sub(r",(\s*[\]}])", r"\1", text)


def parse_jsonc(text: str):
    body = strip_trailing_commas(strip_jsonc(text)).strip()
    if not body:
        return None
    return json.loads(body)


with open(settings_path, "r", encoding="utf-8") as f:
    original = f.read()

try:
    doc = parse_jsonc(original)
except json.JSONDecodeError as e:
    print(f"vscode: failed to parse {settings_path}: {e}", file=sys.stderr)
    raise SystemExit(1)

if doc is None:
    doc = {}
if not isinstance(doc, dict):
    print(f"vscode: expected a JSON object in {settings_path}", file=sys.stderr)
    raise SystemExit(1)

# Disable contrast rewriting (default 4.5 desaturates truecolor).
# https://code.visualstudio.com/docs/terminal/appearance#_minimum-contrast-ratio
doc["terminal.integrated.minimumContrastRatio"] = 1
doc.setdefault("terminal.integrated.lineHeight", 1)
doc.setdefault("terminal.integrated.stickyScroll.enabled", False)
# Pass right-click through to the PTY so sessions-cli gets the event instead of
# VS Code's terminal context menu (rename/delete menu, etc.). Hybrid JS (below)
# restores the IDE menu over the workspace pane via Shift spoof.
# https://code.visualstudio.com/docs/terminal/basics#_right-click-behavior
doc["terminal.integrated.rightClickBehavior"] = "nothing"
# Option+drag forces host text selection even when the PTY app has mouse mode.
doc["terminal.integrated.macOptionClickForcesSelection"] = True

color_custom = doc.get("workbench.colorCustomizations")
if not isinstance(color_custom, dict):
    color_custom = {}
# Match sessions OSC 11 black backdrop (padding shows theme bg otherwise).
color_custom.setdefault("terminal.background", "#000000")
doc["workbench.colorCustomizations"] = color_custom

home = os.path.expanduser("~")
data_root = os.environ.get("SESSIONS_DATA_DIR") or os.path.join(
    os.environ.get("XDG_DATA_HOME", os.path.join(home, ".local", "share")),
    "sessions",
)
asset_dir = os.path.join(data_root, "vscode")
css_path = os.path.join(asset_dir, "sessions-terminal.css")
js_path = os.path.join(asset_dir, "sessions-terminal.js")
os.makedirs(asset_dir, exist_ok=True)

src_css = os.environ.get("SESSIONS_VSCODE_CSS_SRC", "")
src_js = os.environ.get("SESSIONS_VSCODE_JS_SRC", "")
if src_css and os.path.isfile(src_css):
    shutil.copy2(src_css, css_path)
    print(f"vscode: installed terminal CSS → {css_path}")
else:
    print("vscode: no CSS source — skip gutter stylesheet")

if src_js and os.path.isfile(src_js):
    shutil.copy2(src_js, js_path)
    print(f"vscode: installed terminal right-click hybrid JS → {js_path}")
else:
    print("vscode: no hybrid right-click JS source — skip")

imports = doc.get("vscode_custom_css.imports")
if not isinstance(imports, list):
    imports = []
imports = [
    u
    for u in imports
    if not (
        isinstance(u, str)
        and (
            u.rstrip("/").endswith("sessions-terminal.css")
            or u.rstrip("/").endswith("sessions-terminal.js")
        )
    )
]
ordered = []
if os.path.isfile(js_path):
    ordered.append("file://" + js_path)
if os.path.isfile(css_path):
    ordered.append("file://" + css_path)
ordered.extend(imports)
if ordered:
    doc["vscode_custom_css.imports"] = ordered

out = json.dumps(doc, ensure_ascii=False, indent=4) + "\n"
parse_jsonc(out)
tmp = settings_path + ".sessions-tmp"
with open(tmp, "w", encoding="utf-8") as f:
    f.write(out)
os.replace(tmp, settings_path)
print("vscode: terminal.integrated.minimumContrastRatio → 1 (truecolor fidelity)")
print("vscode: terminal.integrated.rightClickBehavior → nothing (pass through to sessions)")
print(
    "vscode: terminal.integrated.macOptionClickForcesSelection → true (Option+drag select)"
)
print(
    "vscode: hybrid right-click — workspace shows IDE menu; sidebar keeps sessions menus"
)
print("vscode: terminal.background → #000000 (match sessions OSC 11)")
print(f"vscode: merged settings into {settings_path}")
print(
    "vscode: reload the window (Developer: Reload Window) for color/right-click changes.\n"
    "        Custom CSS and JS Loader: Enable Custom CSS and JS (once) for gutters +\n"
    "        hybrid right-click. Fallback: Shift+right-click always shows the IDE menu."
)
PY

# macOS Press-and-Hold: same per-app disable as setup-cursor (hold-d / no accent popup).
if [[ "$(uname -s)" == Darwin ]]; then
  if [[ -d "/Applications/Visual Studio Code.app" ]] \
    || defaults read com.microsoft.VSCode >/dev/null 2>&1; then
    defaults write com.microsoft.VSCode ApplePressAndHoldEnabled -bool false 2>/dev/null \
      && echo "vscode: ApplePressAndHoldEnabled=false for VS Code (restart app to apply)" \
      || true
  fi
  if [[ -d "/Applications/Visual Studio Code - Insiders.app" ]] \
    || defaults read com.microsoft.VSCodeInsiders >/dev/null 2>&1; then
    defaults write com.microsoft.VSCodeInsiders ApplePressAndHoldEnabled -bool false 2>/dev/null \
      && echo "vscode: ApplePressAndHoldEnabled=false for VS Code Insiders" \
      || true
  fi
fi
