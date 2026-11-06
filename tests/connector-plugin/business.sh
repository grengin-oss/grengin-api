#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
catalog=$(printenv GRENGIN_CONNECTOR_CATALOG_REPO || printf '%s/../grengin-list' "$root")
bounded=$(printenv GRENGIN_BUSINESS_BOUNDED || printf '0')
if [[ "$bounded" != 1 ]]; then
  exec systemd-run --user --quiet --wait --pipe \
    -p MemoryMax=2G -p MemorySwapMax=0 \
    --setenv=GRENGIN_BUSINESS_BOUNDED=1 \
    --setenv="GRENGIN_CONNECTOR_CATALOG_REPO=$catalog" \
    --setenv="PATH=$PATH" --working-directory="$root" \
    bash "$root/tests/connector-plugin/business.sh"
fi
cd "$root"
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export PYTHONDONTWRITEBYTECODE=1
export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$catalog/master-data/connector-plugins"
cargo test --locked -p connector-server-plugin -j 1 -- --test-threads=1
cargo test --locked -p connector-server-plugin --test providers -j 1 -- --ignored --test-threads=1
cargo test --locked -p connector-server-plugin --test mcp_transport business_regional_setups -j 1 -- --ignored
cargo build --locked -p connector-server-plugin -j 1
python3 -m unittest discover -s "$catalog/scripts/tests" -p 'test_business_connectors.py' -v
python3 -m unittest discover -s "$catalog/scripts/tests" -p 'test_connector_plugins_catalog.py' -v
python3 -m unittest discover -s "$catalog/scripts/tests" -p 'test_read_only_expansion.py' -v
python3 -m unittest discover -s "$catalog/scripts/tests" -p 'test_connector_icons.py' -v
python3 -m unittest discover -s "$catalog/scripts/tests" -p 'test_native_readonly_audit.py' -v
python3 "$catalog/scripts/connector_plugins_catalog.py"
cargo fmt --all --check
git diff --check
git -C "$catalog" diff --check
