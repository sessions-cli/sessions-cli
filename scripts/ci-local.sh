#!/usr/bin/env bash
# Local CI — same gates as .github/workflows/ci.yml and release preflight.
#
# Run this BEFORE committing or releasing. GitHub Actions is a last-resort
# mirror of this script, not the primary test surface.
#
# Usage:
#   ./scripts/ci-local.sh           # full suite (default)
#   ./scripts/ci-local.sh all
#   ./scripts/ci-local.sh fmt
#   ./scripts/ci-local.sh clippy
#   ./scripts/ci-local.sh guards
#   ./scripts/ci-local.sh test
#   ./scripts/ci-local.sh build     # release build --locked (extra; not a GH job)
#
# Make targets:  make check  |  make ci  |  make test
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

mode="${1:-all}"

step() {
  printf '\n== %s ==\n' "$1"
}

die() {
  echo "error: $*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

# Allocate a PTY when needed (CrosstermBackend tests). Linux CI uses
# `script -qec`; macOS BSD `script` uses a different flag order.
run_with_pty() {
  # Prefer a real TTY when the agent/shell already has one.
  if [[ -t 0 && -t 1 ]]; then
    "$@"
    return
  fi
  if script -qec true /dev/null >/dev/null 2>&1; then
    # util-linux (Linux / GitHub Actions)
    script -qec "$*" /dev/null
  elif script -q /dev/null true >/dev/null 2>&1; then
    # BSD (macOS)
    script -q /dev/null "$@"
  else
    echo "warning: cannot allocate PTY via script(1); running bare" >&2
    "$@"
  fi
}

run_fmt() {
  step "fmt (cargo fmt -- --check)"
  need_cmd cargo
  cargo fmt -- --check
}

run_clippy() {
  step "clippy (cargo clippy --locked -- -D warnings)"
  need_cmd cargo
  cargo clippy --locked -- -D warnings
}

run_guards() {
  step "guards (telemetry + private-info)"
  "${ROOT}/scripts/check-telemetry-guards.sh"
  # Private-info scrub is maintainer-only (stripped from public release snapshots).
  # Private tree + ./release.sh preflight always have it; public CI skips.
  if [[ -f "${ROOT}/scripts/check-no-private-info.sh" ]]; then
    "${ROOT}/scripts/check-no-private-info.sh"
  else
    echo "private-info guard skipped (not present in this tree)"
  fi
}

run_test() {
  step "test (cargo test --locked)"
  need_cmd cargo

  # Match CI: telemetry crate first (quoted in YAML), then full suite under a PTY.
  cargo test --locked telemetry::

  # tmux/zsh are required by some daemon tests; warn loudly if missing.
  if ! command -v tmux >/dev/null 2>&1; then
    echo "warning: tmux not on PATH — some daemon tests may fail (CI installs it)" >&2
  fi
  if ! command -v zsh >/dev/null 2>&1; then
    echo "warning: zsh not on PATH — some shell-quoting tests may fail (CI installs it)" >&2
  fi

  if command -v script >/dev/null 2>&1; then
    run_with_pty cargo test --locked
  else
    echo "warning: script(1) not found — running tests without a PTY" >&2
    cargo test --locked
  fi
}

run_build() {
  step "build (cargo build --release --locked)"
  need_cmd cargo
  cargo build --release --locked
}

run_all() {
  run_fmt
  run_clippy
  run_guards
  run_test
}

case "$mode" in
  all|"")
    run_all
    ;;
  fmt)
    run_fmt
    ;;
  clippy)
    run_clippy
    ;;
  guards)
    run_guards
    ;;
  test)
    run_guards
    run_test
    ;;
  build)
    run_build
    ;;
  -h|--help|help)
    sed -n '2,20p' "$0"
    exit 0
    ;;
  *)
    die "unknown mode: $mode (try: all|fmt|clippy|guards|test|build)"
    ;;
esac

printf '\n== ci-local ok (%s) ==\n' "$mode"
