#!/usr/bin/env bash
# Uninstall sessions-cli from a repo checkout or:
#   curl -fsSL https://raw.githubusercontent.com/sessions-cli/sessions-cli/main/uninstall.sh | bash
set -euo pipefail

REPO="${SESSIONS_REPO:-https://github.com/sessions-cli/sessions-cli.git}"
BRANCH="${SESSIONS_BRANCH:-main}"

checkout_root() {
  [[ -n "${BASH_SOURCE[0]:-}" ]] || return 1
  local here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" || return 1
  [[ -f "${here}/bin/uninstall.sh" && -f "${here}/Cargo.toml" ]] || return 1
  printf '%s' "$here"
}

if root="$(checkout_root)"; then
  exec "${root}/bin/uninstall.sh" "$@"
fi

dir="$(mktemp -d)"
cleanup() {
  rm -rf "$dir"
}
trap cleanup EXIT

git clone --depth 1 --branch "$BRANCH" "$REPO" "${dir}/sessions-cli"
"${dir}/sessions-cli/bin/uninstall.sh" "$@"