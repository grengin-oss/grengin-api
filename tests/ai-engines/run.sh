#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
export GRENGIN_ENGINE_INSTALL_TESTS=1
bash tests/security/run.sh "${1:---live}"
