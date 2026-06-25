#!/usr/bin/env bash
# Install sessions-cli from a repo checkout or:
#   curl -fsSL https://raw.githubusercontent.com/sessions-cli/sessions-cli/main/install.sh | bash
set -euo pipefail

REPO="${SESSIONS_REPO:-https://github.com/sessions-cli/sessions-cli.git}"
BRANCH="${SESSIONS_BRANCH:-main}"

checkout_root() {
  [[ -n "${BASH_SOURCE[0]:-}" ]] || return 1
  local here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)" || return 1
  [[ -f "${here}/bin/install.sh" && -f "${here}/Cargo.toml" ]] || return 1
  printf '%s' "$here"
}

if root="$(checkout_root)"; then
  exec "${root}/bin/install.sh" "$@"
fi

dir="$(mktemp -d)"
cleanup() {
  rm -rf "$dir"
}
trap cleanup EXIT

git clone --depth 1 --branch "$BRANCH" "$REPO" "${dir}/sessions-cli"
"${dir}/sessions-cli/bin/install.sh" "$@"