#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
node "$root/tests/connectors/google-staging-ui-check.mjs"
