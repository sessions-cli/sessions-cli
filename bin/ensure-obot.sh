#!/usr/bin/env bash
# Ensure local Obot (MCP control plane) is running for sessions MCP panel.
# Uses Docker image ghcr.io/obot-platform/obot:latest. Soft-fails when Docker
# is unavailable so sessions install still succeeds.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -f "${ROOT}/bin/paths.sh" ]]; then
  # shellcheck source=paths.sh
  . "${ROOT}/bin/paths.sh"
elif [[ -f "$(dirname "$0")/paths.sh" ]]; then
  # shellcheck source=paths.sh
  . "$(dirname "$0")/paths.sh"
else
  sessions_home() { printf '%s' "${HOME:-}"; }
  sessions_config_dir() { printf '%s/.config/sessions' "$1"; }
fi

HOME_DIR="$(sessions_home)"
CONTAINER_NAME="${SESSIONS_OBOT_CONTAINER:-sessions-obot}"
IMAGE="${SESSIONS_OBOT_IMAGE:-ghcr.io/obot-platform/obot:latest}"
PORT="${SESSIONS_OBOT_PORT:-8080}"
BASE_URL="http://127.0.0.1:${PORT}"

QUIET=false
for arg in "$@"; do
  case "$arg" in
    --quiet|-q) QUIET=true ;;
  esac
done

log() {
  if [[ "$QUIET" == false ]]; then
    printf '%s\n' "$*"
  fi
}

step_ok() { log "  ok  obot: $1"; }
step_skip() { log "  —   obot: $1"; }
step_fail() { log "  FAIL obot: $1" >&2; }

docker_bin() {
  if command -v docker >/dev/null 2>&1; then
    command -v docker
    return 0
  fi
  for candidate in \
    /usr/local/bin/docker \
    /opt/homebrew/bin/docker \
    "${HOME_DIR}/.docker/bin/docker"; do
    if [[ -x "$candidate" ]]; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  return 1
}

docker_ready() {
  local d
  d="$(docker_bin)" || return 1
  "$d" info >/dev/null 2>&1
}

try_start_docker_desktop() {
  if [[ "$(uname -s)" != "Darwin" ]]; then
    return 1
  fi
  if [[ -d "/Applications/Docker.app" ]]; then
    log "  …   starting Docker Desktop"
    open -a Docker >/dev/null 2>&1 || true
    local i
    for i in $(seq 1 40); do
      if docker_ready; then
        return 0
      fi
      sleep 1
    done
  fi
  return 1
}

obot_http_up() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsS --connect-timeout 1 --max-time 2 "${BASE_URL}/" >/dev/null 2>&1 \
      || curl -fsS --connect-timeout 1 --max-time 2 -o /dev/null -w '' "${BASE_URL}/" >/dev/null 2>&1 \
      || curl -sS --connect-timeout 1 --max-time 2 -o /dev/null "${BASE_URL}/" >/dev/null 2>&1
    # Any TCP/HTTP response (incl. 401/404) counts as up for our health probe.
    local code
    code="$(curl -sS -o /dev/null -w '%{http_code}' --connect-timeout 1 --max-time 2 "${BASE_URL}/" 2>/dev/null || true)"
    [[ -n "$code" && "$code" != "000" ]]
  else
    return 1
  fi
}

write_obot_config() {
  local token="$1"
  local config_dir
  if command -v sessions_config_dir >/dev/null 2>&1; then
    config_dir="$(sessions_config_dir "$HOME_DIR")"
  else
    config_dir="${HOME_DIR}/.config/sessions"
  fi
  mkdir -p "$config_dir"
  local path="${config_dir}/obot.toml"
  if [[ -f "$path" ]]; then
    # Refresh base_url / ensure enabled; keep existing token if present.
    if grep -q 'bootstrap_token' "$path" 2>/dev/null; then
      step_ok "config ${path}"
      return 0
    fi
  fi
  cat >"$path" <<EOF
# Managed by sessions ensure-obot.sh — Obot MCP control plane
enabled = true
base_url = "${BASE_URL}"
bootstrap_token = "${token}"
open_admin_path = "/mcp-catalog"
EOF
  step_ok "wrote ${path}"
}

docker_sock_mount() {
  if [[ -S /var/run/docker.sock ]]; then
    printf '%s' /var/run/docker.sock
    return 0
  fi
  if [[ -S "${HOME_DIR}/.docker/run/docker.sock" ]]; then
    printf '%s' "${HOME_DIR}/.docker/run/docker.sock"
    return 0
  fi
  printf '%s' /var/run/docker.sock
}

ensure_container() {
  local d token sock
  d="$(docker_bin)"
  token="${SESSIONS_OBOT_TOKEN:-}"
  if [[ -z "$token" ]]; then
    if command -v openssl >/dev/null 2>&1; then
      token="$(openssl rand -hex 16)"
    else
      token="sessions-$(date +%s)-$$"
    fi
  fi
  write_obot_config "$token"

  start_existing() {
    local running
    running="$("$d" inspect -f '{{.State.Running}}' "$CONTAINER_NAME" 2>/dev/null || echo false)"
    if [[ "$running" == "true" ]]; then
      step_ok "container running (${CONTAINER_NAME})"
      return 0
    fi
    log "  …   starting existing container ${CONTAINER_NAME}"
    if "$d" start "$CONTAINER_NAME" >/dev/null 2>&1; then
      step_ok "started ${CONTAINER_NAME}"
      return 0
    fi
    step_fail "could not start ${CONTAINER_NAME}"
    return 1
  }

  if "$d" container inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
    start_existing
    return $?
  fi

  sock="$(docker_sock_mount)"
  log "  …   creating ${CONTAINER_NAME} from ${IMAGE}"
  if "$d" run -d \
    --name "$CONTAINER_NAME" \
    --restart unless-stopped \
    -v "sessions-obot-data:/data" \
    -v "${sock}:/var/run/docker.sock" \
    -p "${PORT}:8080" \
    -e "OBOT_SERVER_ENABLE_AUTHENTICATION=true" \
    -e "OBOT_BOOTSTRAP_TOKEN=${token}" \
    "$IMAGE" >/dev/null; then
    step_ok "created and started ${CONTAINER_NAME} on :${PORT}"
    return 0
  fi

  # Race / leftover: name taken after inspect — try start instead of fail hard.
  if "$d" container inspect "$CONTAINER_NAME" >/dev/null 2>&1; then
    start_existing
    return $?
  fi

  step_fail "docker run failed (is the image pullable?)"
  return 1
}

wait_http() {
  local i
  for i in $(seq 1 30); do
    if obot_http_up; then
      step_ok "listening on ${BASE_URL}"
      return 0
    fi
    sleep 1
  done
  step_skip "container started but HTTP not ready yet (${BASE_URL})"
  return 0
}

main() {
  if obot_http_up; then
    step_ok "already up (${BASE_URL})"
    return 0
  fi

  if ! docker_bin >/dev/null; then
    step_fail "Docker not found — install Docker Desktop, then re-run sessions install or bin/ensure-obot.sh"
    return 1
  fi

  if ! docker_ready; then
    if ! try_start_docker_desktop; then
      step_fail "Docker daemon not running — start Docker Desktop and re-run bin/ensure-obot.sh"
      return 1
    fi
    step_ok "Docker daemon ready"
  fi

  if ! ensure_container; then
    return 1
  fi
  wait_http
  return 0
}

main "$@"
