/**
 * sessions-cli — hybrid terminal right-click for Cursor / VS Code.
 *
 * Setup sets `terminal.integrated.rightClickBehavior` to `nothing` so right-
 * clicks reach the PTY (sessions sidebar menus: rename, end session, notepad).
 * That also hides the IDE terminal context menu (New Terminal, Split, Copy,
 * Copy as HTML, Paste, Clear, Kill Terminal, Toggle Size to Content Width).
 *
 * VS Code already shows that IDE menu on Shift+right-click even when the
 * setting is `nothing`. This script spoofs Shift on right-clicks over the
 * workspace (right of the sessions sidebar) so a normal right-click there
 * restores the IDE menu. Clicks over the left sidebar strip stay un-shifted
 * so sessions-cli still receives them.
 *
 * Loaded via Custom CSS and JS Loader (s-h-a-d-o-w.vscode-custom-css).
 * Installed by bin/setup-cursor.sh / bin/setup-vscode.sh.
 */
(function sessionsTerminalRightClickHybrid() {
  if (globalThis.__sessionsTerminalRightClickHybrid) {
    return;
  }
  globalThis.__sessionsTerminalRightClickHybrid = true;

  /** Default sidebar columns when we cannot read a live width. */
  var DEFAULT_SIDEBAR_COLS = 42;
  /** Never treat more than this fraction of the terminal as sidebar. */
  var MAX_SIDEBAR_FRACTION = 0.48;
  /** Floor so a tiny pane still gets session menus. */
  var MIN_SIDEBAR_PX = 120;

  function terminalRoot(target) {
    if (!target || !target.closest) {
      return null;
    }
    return (
      target.closest(".xterm") ||
      target.closest(".terminal-wrapper") ||
      target.closest(".integrated-terminal") ||
      target.closest(".terminal-editor")
    );
  }

  function measureCellWidth(root) {
    var measure =
      root.querySelector(".xterm-char-measure-element") ||
      root.querySelector(".xterm-char-measure-element span");
    if (measure) {
      var w = measure.getBoundingClientRect().width;
      if (w > 1 && w < 40) {
        return w;
      }
    }
    var screen = root.querySelector(".xterm-screen");
    if (screen) {
      var sw = screen.getBoundingClientRect().width;
      var cols = Number(screen.getAttribute("data-cols") || 0);
      if (!cols && screen.style && screen.style.width) {
        // style width is often in px; fall through
      }
      // xterm canvas/helpers: estimate from rows text length
      var row = root.querySelector(".xterm-rows > div");
      if (row && row.textContent && row.getBoundingClientRect().width > 0) {
        var len = Math.max(row.textContent.length, 1);
        var rw = row.getBoundingClientRect().width / len;
        if (rw > 1 && rw < 40) {
          return rw;
        }
      }
      if (sw > 0 && cols > 0) {
        return sw / cols;
      }
    }
    return 8.4;
  }

  function sidebarWidthPx(root) {
    var rect = root.getBoundingClientRect();
    var cell = measureCellWidth(root);
    var cols = DEFAULT_SIDEBAR_COLS;
    try {
      var stored = Number(
        globalThis.localStorage &&
          globalThis.localStorage.getItem("sessions.sidebarCols")
      );
      if (stored >= 12 && stored <= 120) {
        cols = stored;
      }
    } catch (_) {
      /* ignore */
    }
    var fromCols = cell * (cols + 1);
    var capped = rect.width * MAX_SIDEBAR_FRACTION;
    return Math.max(MIN_SIDEBAR_PX, Math.min(fromCols, capped));
  }

  function isOverSessionsSidebar(event) {
    var root = terminalRoot(event.target);
    if (!root) {
      return false;
    }
    var rect = root.getBoundingClientRect();
    var relX = event.clientX - rect.left;
    if (relX < 0 || relX > rect.width) {
      return false;
    }
    return relX <= sidebarWidthPx(root);
  }

  function forceShiftKey(event) {
    if (event.shiftKey) {
      return;
    }
    try {
      Object.defineProperty(event, "shiftKey", {
        configurable: true,
        enumerable: true,
        get: function () {
          return true;
        },
      });
    } catch (_) {
      /* some hosts seal events; Shift+right-click still works as fallback */
    }
  }

  function onSecondaryPointer(event) {
    // button 2 = right; contextmenu has no button on some paths
    if (event.type === "mousedown" && event.button !== 2) {
      return;
    }
    if (!terminalRoot(event.target)) {
      return;
    }
    // Sidebar: leave shift unset so rightClickBehavior=nothing passes SGR
    // through to sessions-cli. Workspace: spoof shift so VS Code/Cursor shows
    // the IDE terminal context menu (same as Shift+right-click).
    if (!isOverSessionsSidebar(event)) {
      forceShiftKey(event);
    }
  }

  // Capture phase so we run before workbench terminal handlers.
  document.addEventListener("mousedown", onSecondaryPointer, true);
  document.addEventListener("contextmenu", onSecondaryPointer, true);
  document.addEventListener("auxclick", onSecondaryPointer, true);
})();
