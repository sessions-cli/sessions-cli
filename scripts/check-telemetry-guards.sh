#!/usr/bin/env bash
# CI guard: no HTTP client in notify/hooks paths, no secrets in src.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

fail=0

if rg -q 'reqwest' src/notify/ src/hooks/ 2>/dev/null; then
  echo "FAIL: reqwest found in notify or hooks" >&2
  fail=1
fi

if rg -qi 'SUPABASE_DB_PASSWORD|service_role' src/ bin/install.sh 2>/dev/null; then
  echo "FAIL: possible secret embedded in source" >&2
  fail=1
fi

if git ls-files 'infra/supabase/.env.local' 2>/dev/null | grep -q .; then
  echo "FAIL: infra/supabase/.env.local is tracked" >&2
  fail=1
fi

if [[ "$fail" -ne 0 ]]; then
  exit 1
fi

echo "telemetry guards ok"