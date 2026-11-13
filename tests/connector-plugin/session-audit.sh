#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
catalog=$(cd "$root/../grengin-list" && pwd)
mode=${1:-core}
[[ "$mode" == core || "$mode" == full ]] || {
  printf 'Usage: bash tests/connector-plugin/session-audit.sh [core|full]\n' >&2
  exit 2
}
cd "$root"
bash tests/connector-plugin/console.sh runtime
bash tests/connector-plugin/console.sh catalog
bash tests/connector-plugin/oauth-navikt.sh protocols
bash tests/connector-plugin/console.sh check
systemd-run --user --quiet --wait --pipe -p MemoryMax=2G -p MemorySwapMax=0 \
  --setenv="PATH=$PATH" --setenv=CARGO_BUILD_JOBS=1 --setenv=CARGO_INCREMENTAL=0 \
  --setenv=CARGO_PROFILE_DEV_DEBUG=0 --working-directory="$root" \
  cargo clippy --locked -p connector-server-plugin --all-targets --no-deps -j 1
systemd-run --user --quiet --wait --pipe -p MemoryMax=256M -p MemorySwapMax=0 \
  --setenv="PATH=$PATH" --working-directory="$catalog" \
  bash scripts/test_native_expansion.sh
systemd-run --user --quiet --wait --pipe -p MemoryMax=256M -p MemorySwapMax=0 \
  --setenv="PATH=$PATH" --working-directory="$catalog" \
  python3 -m unittest discover -s scripts/tests -p 'test_connector*.py'
cargo fmt --all -- --check
node --check tests/connector-plugin/oauth-navikt.mjs
bash -n tests/connector-plugin/session-audit.sh tests/connector-plugin/console.sh \
  tests/connector-plugin/oauth-navikt.sh tests/connector-plugin/http.sh
git diff --check
git -C "$root/../grengin-list" diff --check
if [[ "$mode" == full ]]; then
  GRENGIN_CONSOLE_MEMORY_MAX=4G bash tests/connector-plugin/console.sh build
  GRENGIN_CONSOLE_MEMORY_MAX=4G bash tests/connector-plugin/console.sh unit
  GRENGIN_CONSOLE_MEMORY_MAX=3G bash tests/connector-plugin/console.sh http
  GRENGIN_CONSOLE_MEMORY_MAX=3G bash tests/connector-plugin/console.sh oauth
  GRENGIN_CONSOLE_MEMORY_MAX=3G bash tests/connector-plugin/console.sh broker
else
  printf '\nCore audit passed. Full backend build/HTTP and real vendor consent remain separate gates.\n'
fi
