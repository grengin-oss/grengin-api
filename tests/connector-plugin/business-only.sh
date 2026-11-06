#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
catalog=${GRENGIN_CONNECTOR_CATALOG_REPO:-$root/../grengin-list}
if [[ ${GRENGIN_BUSINESS_BOUNDED:-0} != 1 ]]; then
  exec systemd-run --user --quiet --wait --pipe -p MemoryMax=2G -p MemorySwapMax=0 \
    --setenv=GRENGIN_BUSINESS_BOUNDED=1 --setenv="PATH=$PATH" \
    --setenv="GRENGIN_CONNECTOR_CATALOG_REPO=$catalog" --working-directory="$root" \
    bash "$root/tests/connector-plugin/business-only.sh"
fi
cd "$root"
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export PYTHONDONTWRITEBYTECODE=1
export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$catalog/master-data/connector-plugins"
cargo test --locked -p connector-server-plugin --test business -j 1 -- --ignored --test-threads=1
python3 -m unittest discover -s "$catalog/scripts/tests" -p test_business_connectors.py -v
