#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
catalog=${GRENGIN_CONNECTOR_CATALOG_ROOT:-$root/../grengin-list}
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$catalog/master-data/connector-plugins"
export PYTHONDONTWRITEBYTECODE=1
cd "$catalog"
python3 scripts/calendar_identifier_fix.py
python3 -m unittest discover -s scripts/tests -p test_calendar_identifier_fix.py -v
cd "$root"
cargo test --locked -p connector-server-plugin --test providers google_calendar_opaque_ids -j2 -- --ignored
if [[ ${1:-} == --live-read ]]; then
  cargo build --locked -p connector-server-plugin --example live_probe -j2
  node tests/connector-plugin/calendar-id-live.mjs
fi
