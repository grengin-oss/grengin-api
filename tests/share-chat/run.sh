#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export GRENGIN_SHARE_CHAT_TESTS=1
# The shared harness restores a read-only dump to disposable Docker PostgreSQL.
bash tests/security/run.sh "${1:---live}"
