#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}" >/dev/null 2>&1
set +a
export GRENGIN_BACKEND_ROOT="$root"
exec node "$root/tests/connector-plugin/live-preflight.mjs" "$@"
