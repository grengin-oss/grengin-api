#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
catalog=${GRENGIN_CONNECTOR_CATALOG_ROOT:-$root/../grengin-list}
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export PYTHONDONTWRITEBYTECODE=1
export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$catalog/master-data/connector-plugins"
cd "$root"
node --check tests/connector-plugin/console-contract.mjs
node --check tests/connector-plugin/query-budget.mjs
node --check tests/connector-plugin/topic-groups.mjs
node --check tests/connector-plugin/oauth-navikt.mjs
cargo test --locked --workspace -j2 --config 'profile.test.package.grengin-api.codegen-units=256' -- --test-threads=1
cargo test --locked -p connector-server-plugin --test providers --test business -j2 -- --ignored --test-threads=1
cargo build --locked -p grengin-api -j2 --config 'profile.dev.package.grengin-api.codegen-units=256'
bash tests/connector-plugin/http.sh
bash tests/connector-plugin/oauth-navikt.sh integration
cd "$catalog"
python3 -m unittest discover -s scripts/tests -q
python3 scripts/connector_plugins_catalog.py
bash scripts/test_grengin_api_distribution.sh
git diff --check
cd "$root"
git diff --check
