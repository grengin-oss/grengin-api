#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
catalog=${GRENGIN_CONNECTOR_CATALOG_ROOT:-$root/../grengin-list}
if [[ ${GRENGIN_CONTENT_BOUNDED:-0} != 1 ]]; then
  exec systemd-run --user --quiet --wait --pipe \
    -p MemoryMax=2G -p MemorySwapMax=0 \
    --setenv=GRENGIN_CONTENT_BOUNDED=1 --setenv="PATH=$PATH" \
    --setenv="GRENGIN_CONNECTOR_CATALOG_ROOT=$catalog" \
    --working-directory="$root" bash "$0"
fi
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export PYTHONDONTWRITEBYTECODE=1
export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$catalog/master-data/connector-plugins"
cd "$root"
cargo test --locked -p llm-plugin -j1
cargo test --locked -p connector-server-plugin -j1 -- --test-threads=1
cargo test --locked -p connector-server-plugin --test providers --test business -j1 -- --ignored --test-threads=1
cargo build --locked -p connector-server-plugin -p llm-plugin -j1
cargo clippy --locked -p connector-server-plugin -p llm-plugin --all-targets --no-deps -j1
cd "$catalog"
python3 -m unittest discover -s scripts/tests -q
python3 scripts/connector_plugins_catalog.py
git diff --check
cd "$root"
git diff --check
