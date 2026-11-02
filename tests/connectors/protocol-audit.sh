#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
node --check "$root/tests/connectors/protocol-audit.mjs"
cd "$website"
node --import tsx "$root/tests/connectors/protocol-audit.mjs"
