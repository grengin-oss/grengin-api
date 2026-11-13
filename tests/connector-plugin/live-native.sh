#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}" >/dev/null 2>&1
set +a
export GRENGIN_BACKEND_ROOT="$root"
if [[ $# -eq 0 ]]; then set -- status; fi
if [[ "$1" == check ]]; then
  "${GRENGIN_NODE_BIN:-node}" --test "$root/tests/connector-plugin/live-callbacks.test.mjs"
fi
exec "${GRENGIN_NODE_BIN:-node}" "$root/tests/connector-plugin/live-native.mjs" "$@"
