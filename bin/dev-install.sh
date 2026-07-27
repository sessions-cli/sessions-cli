#!/usr/bin/env bash
# Build release and deploy sessions (copy + macOS codesign + PATH symlinks).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=paths.sh
. "${ROOT}/bin/paths.sh"
INSTALLER="${ROOT}/bin/install-binary.sh"
BINARY="${ROOT}/target/release/sessions"
NO_BUILD=false

for arg in "$@"; do
  case "$arg" in
    --no-build) NO_BUILD=true ;;
    -h|--help)
      cat <<'EOF'
Usage: bin/dev-install.sh [--no-build]

Build (unless --no-build) and deploy sessions to the canonical install path.
On macOS this copies the binary and signs it — required for `sessions` on PATH.

Equivalent to:
  cargo build --release
  bin/install-binary.sh target/release/sessions
EOF
      exit 0
      ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ ! -x "$INSTALLER" ]]; then
  echo "installer not found: ${INSTALLER}" >&2
  exit 1
fi

if [[ "$NO_BUILD" == false ]]; then
  echo "Building sessions (release)..."
  cargo build --release --manifest-path "${ROOT}/Cargo.toml"
fi

if [[ ! -f "$BINARY" ]]; then
  echo "release binary missing: ${BINARY}" >&2
  exit 1
fi

echo "Deploying and signing..."
"${INSTALLER}" "${BINARY}"

HOME_DIR="$(sessions_home)"
SCRIPTS_DIR="$(sessions_scripts_dir "$HOME_DIR")"
mkdir -p "$SCRIPTS_DIR"
install -m 755 "${ROOT}/bin/ensure-sidebar.sh" "${SCRIPTS_DIR}/ensure-sidebar.sh"
install -m 755 "${ROOT}/bin/restart-sidebar.sh" "${SCRIPTS_DIR}/restart-sidebar.sh"
install -m 755 "${ROOT}/bin/verify-ui-runtime.sh" "${SCRIPTS_DIR}/verify-ui-runtime.sh"
install -m 755 "${ROOT}/bin/ensure-skillshare.sh" "${SCRIPTS_DIR}/ensure-skillshare.sh"
install -m 755 "${ROOT}/bin/ensure-obot.sh" "${SCRIPTS_DIR}/ensure-obot.sh"
install -m 755 "${ROOT}/bin/ensure-companions.sh" "${SCRIPTS_DIR}/ensure-companions.sh"
install -m 644 "${ROOT}/bin/paths.sh" "${SCRIPTS_DIR}/paths.sh"

DEST="$(sessions_install_dir "$HOME_DIR")/sessions"
echo ""
echo "Installed: ${DEST}"
echo "On PATH:   ${HOME_DIR}/.local/bin/sessions -> ${DEST}"
echo "Verify:    sessions --help"