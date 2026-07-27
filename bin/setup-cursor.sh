#!/usr/bin/env bash
# Merge Cursor user keybindings + tasks so ⌘1–⌘0 focus sessions from anywhere in
# Cursor — including the integrated terminal pane, terminal editors, and agent
# terminals. Also installs sidebar resize shortcuts (⌘⌥[ / ⌘⌥]) because the
# IDE edge-drag grip is unreliable in xterm.js.
#
# Speed path (terminal focused): sendSequence injects Meta-digit into the PTY.
# Outer tmux root binds M-1..M-0 → `sessions focus N` (same as Ghostty). This is
# near-instant — VS Code task runner adds hundreds of ms and feels much slower
# than a sidebar click.
#
# Fallback (editor / non-terminal): silent task runs `sessions focus N` so ⌘N
# still works when the terminal is not focused.
#
# Sidebar width (anywhere): ⌘⌥[ / ⌘⌥] run silent tasks → `sessions resize-sidebar
# narrower|wider`. When the sidebar list has focus, plain [ / ] also work inside
# the bar (no Cursor keybind needed).
set -euo pipefail

MARKER_START="// >>> sessions-cli cursor keybinds >>>"
MARKER_END="// <<< sessions-cli cursor keybinds <<<"
TASKS_MARKER_START="// >>> sessions-cli cursor tasks >>>"
TASKS_MARKER_END="// <<< sessions-cli cursor tasks <<<"

cursor_user_dir() {
  if [[ "$(uname -s)" == Darwin ]]; then
    printf '%s\n' "${HOME}/Library/Application Support/Cursor/User"
  else
    printf '%s\n' "${HOME}/.config/Cursor/User"
  fi
}

USER_DIR="$(cursor_user_dir)"
KEYBINDINGS="${USER_DIR}/keybindings.json"
TASKS="${USER_DIR}/tasks.json"
SETTINGS="${USER_DIR}/settings.json"
mkdir -p "${USER_DIR}"

if [[ ! -f "$KEYBINDINGS" ]]; then
  printf '%s\n' '// Place your key bindings in this file to override the defaults' '[' ']' >"$KEYBINDINGS"
fi
if [[ ! -f "$TASKS" ]]; then
  printf '%s\n' '{' '  "version": "2.0.0",' '  "tasks": []' '}' >"$TASKS"
fi
if [[ ! -f "$SETTINGS" ]]; then
  printf '%s\n' '{' '}' >"$SETTINGS"
fi

# Prefer installed sessions on PATH, then the standard install location.
SESSIONS_BIN="$(command -v sessions 2>/dev/null || true)"
if [[ -z "$SESSIONS_BIN" && -x "${HOME}/.local/bin/sessions" ]]; then
  SESSIONS_BIN="${HOME}/.local/bin/sessions"
fi
if [[ -z "$SESSIONS_BIN" && -x "${HOME}/.local/share/sessions/bin/sessions" ]]; then
  SESSIONS_BIN="${HOME}/.local/share/sessions/bin/sessions"
fi
if [[ -z "$SESSIONS_BIN" ]]; then
  SESSIONS_BIN="sessions"
fi

# Keep terminal cells in step with editor text (same size). 14 is the default
# sessions UX step up from Cursor's common 12–13 without jumping to zoom 1.
SESSIONS_TERMINAL_FONT_SIZE="${SESSIONS_TERMINAL_FONT_SIZE:-14}"

export SESSIONS_CURSOR_KEYBINDINGS="$KEYBINDINGS"
export SESSIONS_CURSOR_TASKS="$TASKS"
export SESSIONS_CURSOR_SETTINGS="$SETTINGS"
export SESSIONS_CURSOR_MARKER_START="$MARKER_START"
export SESSIONS_CURSOR_MARKER_END="$MARKER_END"
export SESSIONS_CURSOR_TASKS_MARKER_START="$TASKS_MARKER_START"
export SESSIONS_CURSOR_TASKS_MARKER_END="$TASKS_MARKER_END"
export SESSIONS_BIN
export SESSIONS_TERMINAL_FONT_SIZE

# Resolve CSS source from this checkout (portable; no hardcoded user paths).
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
export SESSIONS_SETUP_CURSOR_DIR="$(cd "$(dirname "$0")" && pwd)"
export SESSIONS_CURSOR_CSS_SRC="${ROOT}/integrations/cursor/sessions-terminal.css"
export SESSIONS_CURSOR_JS_SRC="${ROOT}/integrations/cursor/sessions-terminal.js"

python3 - <<'PY'
import json
import os
import re
import sys

kb_path = os.environ["SESSIONS_CURSOR_KEYBINDINGS"]
tasks_path = os.environ["SESSIONS_CURSOR_TASKS"]
settings_path = os.environ["SESSIONS_CURSOR_SETTINGS"]
marker_start = os.environ["SESSIONS_CURSOR_MARKER_START"]
marker_end = os.environ["SESSIONS_CURSOR_MARKER_END"]
tasks_marker_start = os.environ["SESSIONS_CURSOR_TASKS_MARKER_START"]
tasks_marker_end = os.environ["SESSIONS_CURSOR_TASKS_MARKER_END"]
sessions_bin = os.environ["SESSIONS_BIN"]
try:
    terminal_font_size = int(os.environ.get("SESSIONS_TERMINAL_FONT_SIZE", "14"))
except ValueError:
    terminal_font_size = 14
if terminal_font_size < 10 or terminal_font_size > 32:
    terminal_font_size = 14

# Any terminal surface in Cursor (panel, editor tab, agent terminal).
TERMINAL_WHEN = (
    "terminalFocus || terminalEditorFocus || terminalFocusInAny "
    "|| terminalTabsFocus || terminalTabFocus"
)

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


def drop_marker_block(text: str, start: str, end: str) -> str:
    return re.sub(
        re.escape(start) + r".*?" + re.escape(end),
        "",
        text,
        flags=re.S,
    )


def parse_jsonc(text: str):
    body = strip_trailing_commas(strip_jsonc(text)).strip()
    if not body:
        return None
    return json.loads(body)


def task_label(ordinal: int) -> str:
    return f"sessions: focus {ordinal}"


RESIZE_NARROWER_LABEL = "sessions: resize sidebar narrower"
RESIZE_WIDER_LABEL = "sessions: resize sidebar wider"
# Prefer ⌘⌥ over ⌘⇧ so we do not steal editor fold / navigate defaults as often.
# Avoid bare ⌘[ / ⌘] (navigate back/forward).
RESIZE_KEYS = ("cmd+alt+[", "cmd+alt+]", "ctrl+alt+[", "ctrl+alt+]")


unbind_cmds = {
    "workbench.action.focusFirstEditorGroup",
    "workbench.action.focusSecondEditorGroup",
    "workbench.action.focusThirdEditorGroup",
    "workbench.action.focusFourthEditorGroup",
    "workbench.action.focusFifthEditorGroup",
    "workbench.action.focusSixthEditorGroup",
    "workbench.action.focusSeventhEditorGroup",
    "workbench.action.focusEighthEditorGroup",
    "workbench.action.focusLastEditorGroup",
    "workbench.action.zoomReset",
    "workbench.action.openEditorAtIndex1",
    "workbench.action.openEditorAtIndex2",
    "workbench.action.openEditorAtIndex3",
    "workbench.action.openEditorAtIndex4",
    "workbench.action.openEditorAtIndex5",
    "workbench.action.openEditorAtIndex6",
    "workbench.action.openEditorAtIndex7",
    "workbench.action.openEditorAtIndex8",
    "workbench.action.openEditorAtIndex9",
}
digit_keys = {f"cmd+{d}" for d in "0123456789"} | {f"ctrl+{d}" for d in "0123456789"}
sessions_send = "workbench.action.terminal.sendSequence"
run_task = "workbench.action.tasks.runTask"

ordinals = [
    ("1", 1, "workbench.action.focusFirstEditorGroup"),
    ("2", 2, "workbench.action.focusSecondEditorGroup"),
    ("3", 3, "workbench.action.focusThirdEditorGroup"),
    ("4", 4, "workbench.action.focusFourthEditorGroup"),
    ("5", 5, "workbench.action.focusFifthEditorGroup"),
    ("6", 6, "workbench.action.focusSixthEditorGroup"),
    ("7", 7, "workbench.action.focusSeventhEditorGroup"),
    ("8", 8, "workbench.action.focusEighthEditorGroup"),
    ("9", 9, "workbench.action.focusLastEditorGroup"),
    ("0", 10, "workbench.action.zoomReset"),
]

# --- keybindings.json ---
with open(kb_path, "r", encoding="utf-8") as f:
    kb_original = f.read()

kb_stripped = drop_marker_block(kb_original, marker_start, marker_end)
try:
    existing = parse_jsonc(kb_stripped)
except json.JSONDecodeError as e:
    print(f"cursor: failed to parse {kb_path}: {e}", file=sys.stderr)
    sys.exit(1)

if existing is None:
    existing = []
if not isinstance(existing, list):
    print(f"cursor: expected a JSON array in {kb_path}", file=sys.stderr)
    sys.exit(1)


def is_stale_sessions_entry(entry: object) -> bool:
    if not isinstance(entry, dict):
        return False
    key = str(entry.get("key", "")).lower().replace(" ", "")
    cmd = str(entry.get("command", ""))
    # Prior sessions resize keybinds (re-merge on every setup).
    if key in {k.replace(" ", "") for k in RESIZE_KEYS} and cmd == run_task:
        args = entry.get("args")
        if isinstance(args, str) and args.startswith("sessions: resize sidebar "):
            return True
    if key not in digit_keys:
        return False
    if cmd == sessions_send:
        args = entry.get("args") or {}
        text = str(args.get("text", ""))
        if text.startswith("\x1b") and len(text) == 2 and text[1] in "0123456789":
            return True
    if cmd == run_task:
        args = entry.get("args")
        if isinstance(args, str) and args.startswith("sessions: focus "):
            return True
    if cmd.startswith("-") and cmd[1:] in unbind_cmds:
        return True
    return False


kept = [e for e in existing if not is_stale_sessions_entry(e)]
focus_unbinds = {f"-{c}" for c in unbind_cmds}
kept = [
    e
    for e in kept
    if not (
        isinstance(e, dict)
        and str(e.get("key", "")).lower().replace(" ", "") in digit_keys
        and str(e.get("command", "")) in focus_unbinds
    )
]

block: list[dict] = []
for key_digit, ordinal, default_cmd in ordinals:
    # Unbind Cursor defaults that steal ⌘N / ⌃N.
    block.append({"key": f"cmd+{key_digit}", "command": f"-{default_cmd}"})
    block.append({"key": f"ctrl+{key_digit}", "command": f"-{default_cmd}"})
    if key_digit != "0":
        block.append(
            {
                "key": f"cmd+{key_digit}",
                "command": f"-workbench.action.openEditorAtIndex{key_digit}",
            }
        )
        block.append(
            {
                "key": f"ctrl+{key_digit}",
                "command": f"-workbench.action.openEditorAtIndex{key_digit}",
            }
        )
    label = task_label(ordinal)
    # Meta-digit as ESC+digit — outer tmux binds M-N → sessions focus (fast).
    meta_text = f"\u001b{key_digit}"
    for mod in ("cmd", "ctrl"):
        # Fast path: terminal surfaces inject Meta into the sessions PTY.
        block.append(
            {
                "key": f"{mod}+{key_digit}",
                "command": sessions_send,
                "args": {"text": meta_text},
                "when": TERMINAL_WHEN,
            }
        )
        # Slow fallback: editor / non-terminal — daemon focus via silent task.
        block.append(
            {
                "key": f"{mod}+{key_digit}",
                "command": run_task,
                "args": label,
                "when": f"!({TERMINAL_WHEN})",
            }
        )

# Sidebar resize — works with terminal or editor focus (task → sessions CLI).
# Bar-focused plain [ / ] still work without these when the PTY has focus.
for key, label in (
    ("cmd+alt+[", RESIZE_NARROWER_LABEL),
    ("cmd+alt+]", RESIZE_WIDER_LABEL),
    ("ctrl+alt+[", RESIZE_NARROWER_LABEL),
    ("ctrl+alt+]", RESIZE_WIDER_LABEL),
):
    block.append(
        {
            "key": key,
            "command": run_task,
            "args": label,
        }
    )


def write_keybindings(path: str, original: str, kept_entries: list, block_entries: list) -> None:
    header = "// Place your key bindings in this file to override the defaults\n"
    lead = original.lstrip("\ufeff")
    if lead.startswith("//"):
        header = lead.splitlines()[0] + "\n"

    parts = [header, "[\n"]
    for i, entry in enumerate(kept_entries):
        dumped = json.dumps(entry, ensure_ascii=False, indent=4)
        indented = "\n".join(("    " + line if line else line) for line in dumped.splitlines())
        parts.append(indented)
        parts.append(",\n" if i < len(kept_entries) - 1 or block_entries else "\n")

    if block_entries:
        if kept_entries and not parts[-1].endswith(",\n"):
            if parts[-1] == "\n":
                parts[-1] = ",\n"
            elif parts[-1].endswith("\n"):
                parts[-1] = parts[-1][:-1] + ",\n"
        parts.append(f"    {marker_start}\n")
        for i, entry in enumerate(block_entries):
            dumped = json.dumps(entry, ensure_ascii=False, indent=4)
            indented = "\n".join(
                ("    " + line if line else line) for line in dumped.splitlines()
            )
            parts.append(indented)
            parts.append(",\n" if i < len(block_entries) - 1 else "\n")
        parts.append(f"    {marker_end}\n")

    parts.append("]\n")
    text = "".join(parts)
    parse_jsonc(text)  # validate
    tmp = path + ".sessions-tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        f.write(text)
    os.replace(tmp, path)


write_keybindings(kb_path, kb_original, kept, block)
print(f"cursor: merged sessions keybinds into {kb_path}")

# --- tasks.json ---
with open(tasks_path, "r", encoding="utf-8") as f:
    tasks_original = f.read()

tasks_stripped = drop_marker_block(tasks_original, tasks_marker_start, tasks_marker_end)
try:
    tasks_doc = parse_jsonc(tasks_stripped)
except json.JSONDecodeError as e:
    print(f"cursor: failed to parse {tasks_path}: {e}", file=sys.stderr)
    sys.exit(1)

if tasks_doc is None:
    tasks_doc = {"version": "2.0.0", "tasks": []}
if not isinstance(tasks_doc, dict):
    print(f"cursor: expected a JSON object in {tasks_path}", file=sys.stderr)
    sys.exit(1)

existing_tasks = tasks_doc.get("tasks") or []
if not isinstance(existing_tasks, list):
    existing_tasks = []

sessions_labels = {task_label(o) for _, o, _ in ordinals} | {
    RESIZE_NARROWER_LABEL,
    RESIZE_WIDER_LABEL,
}
kept_tasks = [
    t
    for t in existing_tasks
    if not (isinstance(t, dict) and str(t.get("label", "")) in sessions_labels)
]

silent = {
    "reveal": "never",
    "revealProblems": "never",
    "echo": False,
    "focus": False,
    "panel": "shared",
    "showReuseMessage": False,
    "clear": False,
    "close": True,
}

new_tasks = []
for _digit, ordinal, _cmd in ordinals:
    new_tasks.append(
        {
            "label": task_label(ordinal),
            "type": "process",
            "command": sessions_bin,
            "args": ["focus", str(ordinal)],
            "presentation": silent,
            "problemMatcher": [],
        }
    )
for label, direction in (
    (RESIZE_NARROWER_LABEL, "narrower"),
    (RESIZE_WIDER_LABEL, "wider"),
):
    new_tasks.append(
        {
            "label": label,
            "type": "process",
            "command": sessions_bin,
            "args": ["resize-sidebar", direction],
            "presentation": silent,
            "problemMatcher": [],
        }
    )

# Rebuild tasks.json as JSONC with markers around our tasks for idempotent updates.
header_lines = []
lead = tasks_original.lstrip("\ufeff")
if lead.startswith("//"):
    header_lines.append(lead.splitlines()[0])

out = []
if header_lines:
    out.append(header_lines[0] + "\n")
out.append("{\n")
out.append('  "version": "2.0.0",\n')
out.append('  "tasks": [\n')

all_tasks = kept_tasks + new_tasks
# Write kept tasks first, then marker + sessions tasks.
for i, task in enumerate(kept_tasks):
    dumped = json.dumps(task, ensure_ascii=False, indent=2)
    indented = "\n".join(("    " + line if line else line) for line in dumped.splitlines())
    out.append(indented)
    out.append(",\n" if i < len(kept_tasks) - 1 or new_tasks else "\n")

if new_tasks:
    if kept_tasks and not out[-1].endswith(",\n"):
        if out[-1] == "\n":
            out[-1] = ",\n"
        elif out[-1].endswith("\n"):
            out[-1] = out[-1][:-1] + ",\n"
    out.append(f"    {tasks_marker_start}\n")
    for i, task in enumerate(new_tasks):
        dumped = json.dumps(task, ensure_ascii=False, indent=2)
        indented = "\n".join(
            ("    " + line if line else line) for line in dumped.splitlines()
        )
        out.append(indented)
        out.append(",\n" if i < len(new_tasks) - 1 else "\n")
    out.append(f"    {tasks_marker_end}\n")

out.append("  ]\n")
out.append("}\n")
text = "".join(out)
parse_jsonc(text)
tmp = tasks_path + ".sessions-tmp"
with open(tmp, "w", encoding="utf-8") as f:
    f.write(text)
os.replace(tmp, tasks_path)
print(f"cursor: merged sessions tasks into {tasks_path}")
print(f"cursor: sessions binary → {sessions_bin}")
print(f"cursor: ⌘1–0 when → {TERMINAL_WHEN}")
print("cursor: sidebar resize → ⌘⌥[ narrower / ⌘⌥] wider (also plain [ / ] in sidebar)")

# --- settings.json (terminal font for sessions UX) ---
SETTINGS_MARKER_START = "// >>> sessions-cli cursor settings >>>"
SETTINGS_MARKER_END = "// <<< sessions-cli cursor settings <<<"

with open(settings_path, "r", encoding="utf-8") as f:
    settings_original = f.read()

# Drop a previous sessions-managed comment block if present (we rewrite keys below).
settings_body = drop_marker_block(
    settings_original, SETTINGS_MARKER_START, SETTINGS_MARKER_END
)
try:
    settings_doc = parse_jsonc(settings_body)
except json.JSONDecodeError as e:
    print(f"cursor: failed to parse {settings_path}: {e}", file=sys.stderr)
    sys.exit(1)

if settings_doc is None:
    settings_doc = {}
if not isinstance(settings_doc, dict):
    print(f"cursor: expected a JSON object in {settings_path}", file=sys.stderr)
    sys.exit(1)

# Keep editor + terminal on the same type size and family (sessions UX parity).
editor_family = settings_doc.get("editor.fontFamily")
if isinstance(editor_family, str) and editor_family.strip():
    settings_doc["terminal.integrated.fontFamily"] = editor_family

prev_term = settings_doc.get("terminal.integrated.fontSize")
prev_editor = settings_doc.get("editor.fontSize")
settings_doc["terminal.integrated.fontSize"] = terminal_font_size
settings_doc["editor.fontSize"] = terminal_font_size
# 1.0 keeps row height tight — 1.2 can look like a soft top/bottom bar.
settings_doc["terminal.integrated.lineHeight"] = 1

# Reduce terminal chrome that reads as empty bars around sessions.
# (The hard left 20px gutter is CSS — see integrations/cursor/sessions-terminal.css.)
settings_doc["terminal.integrated.tabs.hideCondition"] = "singleTerminal"
settings_doc["terminal.integrated.stickyScroll.enabled"] = False

# Color fidelity: VS Code/Cursor default minimumContrastRatio is 4.5, which
# rewrites every cell FG/BG until WCAG contrast is met. That desaturates
# truecolor (sessions RGB greys, greens, warm accents) so the palette feels
# "reduced". 1 disables rewriting — original SGR/truecolor is shown.
# https://code.visualstudio.com/docs/terminal/appearance#_minimum-contrast-ratio
settings_doc["terminal.integrated.minimumContrastRatio"] = 1

# Pass right-click through to the PTY so sessions-cli gets the event instead of
# Cursor/VS Code's terminal context menu (rename/delete menu, etc.).
# https://code.visualstudio.com/docs/terminal/basics#_right-click-behavior
settings_doc["terminal.integrated.rightClickBehavior"] = "nothing"
# Option+drag forces host text selection even when the PTY app has mouse mode
# (Grok/OpenCode). Fallback when app-native select is flaky; ⌘C then copies.
settings_doc["terminal.integrated.macOptionClickForcesSelection"] = True

# Match sessions OSC 11 backdrop so padding/chrome isn't theme-grey.
color_custom = settings_doc.get("workbench.colorCustomizations")
if not isinstance(color_custom, dict):
    color_custom = {}
color_custom.setdefault("terminal.background", "#000000")
settings_doc["workbench.colorCustomizations"] = color_custom

# Tab label: Cursor/VS Code `${process}` uses the *real executable name*
# (macOS ucomm), which stays "tmux" even after `exec -a sessions` — argv0 is
# ignored. Pin the tab title so sessions attach does not show as "tmux".
# Override in settings.json if you use this terminal for non-sessions work.
settings_doc["terminal.integrated.tabs.title"] = "sessions"
settings_doc.setdefault("terminal.integrated.tabs.description", "${cwdFolder}")

# Install edge-to-edge CSS + hybrid right-click JS (workspace → IDE menu).
home = os.path.expanduser("~")
data_root = os.environ.get("SESSIONS_DATA_DIR") or os.path.join(
    os.environ.get("XDG_DATA_HOME", os.path.join(home, ".local", "share")),
    "sessions",
)
asset_dir = os.path.join(data_root, "cursor")
css_path = os.path.join(asset_dir, "sessions-terminal.css")
js_path = os.path.join(asset_dir, "sessions-terminal.js")
os.makedirs(asset_dir, exist_ok=True)

import shutil

script_dir = os.environ.get("SESSIONS_SETUP_CURSOR_DIR", "")
here = os.environ.get("PWD", "")


def resolve_asset(env_key: str, filename: str) -> str:
    env_path = os.environ.get(env_key, "")
    if env_path and os.path.isfile(env_path):
        return env_path
    candidates = []
    if script_dir:
        candidates.append(
            os.path.normpath(
                os.path.join(script_dir, "..", "integrations", "cursor", filename)
            )
        )
    if here:
        candidates.append(os.path.join(here, "integrations", "cursor", filename))
    return next((p for p in candidates if os.path.isfile(p)), "")


src_css = resolve_asset("SESSIONS_CURSOR_CSS_SRC", "sessions-terminal.css")
src_js = resolve_asset("SESSIONS_CURSOR_JS_SRC", "sessions-terminal.js")

if src_css:
    shutil.copy2(src_css, css_path)
    print(f"cursor: installed terminal CSS → {css_path}")
else:
    # Minimal inline fallback if the repo file is missing.
    with open(css_path, "w", encoding="utf-8") as f:
        f.write(
            """/* sessions-cli terminal gutter kill (fallback) */
.monaco-workbench .xterm { padding: 0 !important; }
.monaco-workbench .xterm .xterm-scrollable-element {
  margin-left: 0 !important; padding-left: 0 !important;
}
"""
        )
    print(f"cursor: wrote fallback terminal CSS → {css_path}")

if src_js:
    shutil.copy2(src_js, js_path)
    print(f"cursor: installed terminal right-click hybrid JS → {js_path}")
else:
    print("cursor: no hybrid right-click JS source — skip (Shift+right-click still shows IDE menu)")

imports = settings_doc.get("vscode_custom_css.imports")
if not isinstance(imports, list):
    imports = []
# Drop stale sessions CSS/JS entries, then prepend ours (JS first).
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
ordered.append("file://" + css_path)
ordered.extend(imports)
settings_doc["vscode_custom_css.imports"] = ordered

# Prefer stable key order for the keys we care about; leave others as-is via re-dump.
# Re-dumping the whole object is fine — Cursor settings are a flat JSON object.
out_settings = json.dumps(settings_doc, ensure_ascii=False, indent=4) + "\n"
# Validate
parse_jsonc(out_settings)
tmp = settings_path + ".sessions-tmp"
with open(tmp, "w", encoding="utf-8") as f:
    f.write(out_settings)
os.replace(tmp, settings_path)
print(
    f"cursor: terminal.integrated.fontSize {prev_term!r} → {terminal_font_size}; "
    f"editor.fontSize {prev_editor!r} → {terminal_font_size} "
    f"(override with SESSIONS_TERMINAL_FONT_SIZE)"
)
print("cursor: terminal.integrated.rightClickBehavior → nothing (pass through to sessions)")
print(
    "cursor: terminal.integrated.macOptionClickForcesSelection → true (Option+drag select)"
)
print(
    "cursor: hybrid right-click — workspace shows IDE menu; sidebar keeps sessions menus"
)
print(f"cursor: merged sessions settings into {settings_path}")
print(
    "cursor: left/top gutters + hybrid right-click require Custom CSS and JS Loader.\n"
    "        To activate (once):\n"
    "          1. Extension: Custom CSS and JS Loader (s-h-a-d-o-w.vscode-custom-css)\n"
    "          2. Command Palette → “Enable Custom CSS and JS”\n"
    "          3. Reload Cursor (allow patching if prompted)\n"
    f"        stylesheet: {css_path}\n"
    f"        script:     {js_path if os.path.isfile(js_path) else '(missing)'}\n"
    "        Fallback without JS: Shift+right-click always shows the IDE menu."
)
PY

# Best-effort install of the Custom CSS loader (Cursor Open VSX id).
if [[ -x "/Applications/Cursor.app/Contents/Resources/app/bin/cursor" ]]; then
  CURSOR_BIN="/Applications/Cursor.app/Contents/Resources/app/bin/cursor"
elif command -v cursor >/dev/null 2>&1; then
  CURSOR_BIN="$(command -v cursor)"
else
  CURSOR_BIN=""
fi
if [[ -n "$CURSOR_BIN" ]]; then
  if ! "$CURSOR_BIN" --list-extensions 2>/dev/null | grep -qi 'custom-css'; then
    echo "cursor: installing Custom CSS and JS Loader extension…"
    "$CURSOR_BIN" --install-extension s-h-a-d-o-w.vscode-custom-css >/dev/null 2>&1 \
      && echo "cursor: extension installed — run “Enable Custom CSS and JS” once from the Command Palette" \
      || echo "cursor: could not auto-install extension; install s-h-a-d-o-w.vscode-custom-css manually"
  else
    echo "cursor: Custom CSS loader extension already installed"
  fi
fi

# macOS Press-and-Hold shows accent/language alternatives for letter keys and
# blocks key-repeat — that breaks hold-d close mode and pops a D-alternatives
# dropdown when holding d (or ⌘D paths that still emit d). Disable per-app only.
if [[ "$(uname -s)" == Darwin ]]; then
  disable_press_and_hold() {
    local bundle_id="$1"
    local label="$2"
    defaults write "$bundle_id" ApplePressAndHoldEnabled -bool false 2>/dev/null \
      && echo "cursor: ApplePressAndHoldEnabled=false for ${label} (restart app to apply)" \
      || true
  }
  if [[ -d "/Applications/Cursor.app" ]] || defaults read com.todesktop.230313mzl4w4u92 >/dev/null 2>&1; then
    disable_press_and_hold com.todesktop.230313mzl4w4u92 "Cursor"
  fi
  if [[ -d "/Applications/Visual Studio Code.app" ]] \
    || defaults read com.microsoft.VSCode >/dev/null 2>&1; then
    disable_press_and_hold com.microsoft.VSCode "VS Code"
  fi
  if [[ -d "/Applications/Visual Studio Code - Insiders.app" ]] \
    || defaults read com.microsoft.VSCodeInsiders >/dev/null 2>&1; then
    disable_press_and_hold com.microsoft.VSCodeInsiders "VS Code Insiders"
  fi
fi
