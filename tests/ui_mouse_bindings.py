#!/usr/bin/env python3
"""Regression test for root tmux mouse bindings.

Creates an isolated tmux server with a nested workspace pane, installs the
binding strings passed in via environment variables, attaches a pty client,
sends synthetic SGR mouse events, and fails if tmux reports a syntax error.
"""
import os
import pty
import select
import subprocess
import sys
import time
import tty
import termios


def tmux(sock, *args, capture=False):
    cmd = ["tmux", "-L", sock] + list(args)
    if capture:
        return subprocess.run(cmd, capture_output=True, text=True).stdout.strip()
    subprocess.run(cmd, check=False)


def click_at(master, col, row):
    # SGR mouse: 0 = primary press, 0+32 = drag, 64/65 = wheel up/down.
    os.write(master, f"\x1b[<0;{col};{row}M".encode())
    time.sleep(0.05)
    os.write(master, f"\x1b[<0;{col};{row}m".encode())
    time.sleep(0.05)


def drag_at(master, col1, row1, col2, row2):
    os.write(master, f"\x1b[<0;{col1};{row1}M".encode())
    time.sleep(0.05)
    os.write(master, f"\x1b[<32;{col2};{row2}M".encode())
    time.sleep(0.05)
    os.write(master, f"\x1b[<0;{col2};{row2}m".encode())
    time.sleep(0.05)


def wheel_at(master, col, row, up=True):
    code = 64 if up else 65
    os.write(master, f"\x1b[<{code};{col};{row}M".encode())
    time.sleep(0.05)


def has_syntax_error(sock):
    msgs = tmux(sock, "show-messages", "-t", "test-ui", capture=True)
    return "syntax error" in msgs.lower()


def main():
    click = os.environ.get("CLICK", "")
    up = os.environ.get("UP", "")
    drag = os.environ.get("DRAG", "")
    wheel_up = os.environ.get("WHEEL_UP", "")
    wheel_down = os.environ.get("WHEEL_DOWN", "")
    if not all([click, up, drag, wheel_up, wheel_down]):
        print("Missing binding environment variables", file=sys.stderr)
        sys.exit(1)

    sock = f"sessions-mouse-test-{os.getpid()}"
    subprocess.run(["tmux", "-L", sock, "kill-server"], capture_output=True)
    time.sleep(0.2)

    tmux(sock, "new-session", "-d", "-s", "test-agents", "-n", "agents", "sleep 1000")
    attach = f"exec env -u TMUX tmux -L {sock} attach-session -t test-agents"
    tmux(sock, "new-session", "-d", "-s", "test-ui", "-n", "ui", f"/bin/zsh -lc {attach}")
    tmux(sock, "split-window", "-h", "-b", "-t", "test-ui:ui.0", "-l", "40", "sleep 1000")
    tmux(sock, "set-option", "-t", "test-ui", "mouse", "on")
    tmux(sock, "set-option", "-t", "test-agents", "mouse", "on")

    tmux(sock, "bind-key", "-T", "root", "MouseDown1Pane", click)
    tmux(sock, "bind-key", "-T", "root", "MouseUp1Pane", up)
    tmux(sock, "bind-key", "-T", "root", "MouseDrag1Pane", drag)
    tmux(sock, "bind-key", "-T", "root", "WheelUpPane", wheel_up)
    tmux(sock, "bind-key", "-T", "root", "WheelDownPane", wheel_down)

    master, slave = pty.openpty()
    stderr_path = f"/tmp/sessions-mouse-test-{os.getpid()}.stderr"
    pid = os.fork()
    if pid == 0:
        os.close(master)
        os.setsid()
        os.dup2(slave, 0)
        os.dup2(slave, 1)
        os.dup2(slave, 2)
        os.close(slave)
        # Close inherited file descriptors so the tmux client starts cleanly
        # when this test is invoked from a multi-threaded cargo test process.
        try:
            os.closerange(3, 1024)
        except Exception:
            pass
        # Redirect stderr to a file for post-mortem debugging.
        sys.stderr = open(stderr_path, "w")
        os.execlp("tmux", "tmux", "-L", sock, "attach-session", "-t", "test-ui")
        sys.exit(1)
    os.close(slave)
    tty.setraw(master, termios.TCSANOW)

    # Wait for the attached client to appear; under parallel cargo test the
    # tmux client may take a moment to start.
    client_tty = None
    for _ in range(50):
        client_tty = tmux(sock, "list-clients", "-t", "test-ui", "-F", "#{client_tty}", capture=True)
        if client_tty:
            break
        time.sleep(0.1)
    if not client_tty:
        print("FAIL: tmux client never attached to test-ui", file=sys.stderr)
        print(f"session list:\n{tmux(sock, 'list-sessions', capture=True)}", file=sys.stderr)
        if os.path.exists(stderr_path):
            with open(stderr_path) as f:
                print(f"client stderr:\n{f.read()}", file=sys.stderr)
        # Best-effort cleanup.
        try:
            os.kill(pid, 15)
        except Exception:
            pass
        os.waitpid(pid, 0)
        subprocess.run(["tmux", "-L", sock, "kill-server"], capture_output=True)
        try:
            os.remove(stderr_path)
        except Exception:
            pass
        sys.exit(1)

    try:
        # Right pane is around columns 42-80.
        click_at(master, 60, 5)
        if has_syntax_error(sock):
            print("FAIL: syntax error after click", file=sys.stderr)
            sys.exit(1)

        drag_at(master, 60, 5, 70, 8)
        if has_syntax_error(sock):
            print("FAIL: syntax error after drag", file=sys.stderr)
            sys.exit(1)

        wheel_at(master, 60, 5, up=True)
        wheel_at(master, 60, 5, up=False)
        if has_syntax_error(sock):
            print("FAIL: syntax error after wheel", file=sys.stderr)
            sys.exit(1)
    finally:
        try:
            os.kill(pid, 15)
        except Exception:
            pass
        _, status = os.waitpid(pid, 0)
        if status != 0:
            print(f"tmux attach child exited with status {status}", file=sys.stderr)
        subprocess.run(["tmux", "-L", sock, "kill-server"], capture_output=True)
        try:
            os.remove(stderr_path)
        except Exception:
            pass

    print("ok")


if __name__ == "__main__":
    main()
