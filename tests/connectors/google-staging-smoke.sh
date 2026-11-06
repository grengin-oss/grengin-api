#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}" >/dev/null 2>&1
set +a
node "$root/tests/connectors/google-staging-smoke.mjs"
