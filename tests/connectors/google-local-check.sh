#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
node "$root/tests/connectors/google-local-check.mjs"
