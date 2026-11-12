#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
mode=${1:-mock}
case "$mode" in backend-suite|suite|mock|catalog|catalog-all|validate|live) ;; *) printf 'Usage: bash tests/providers/gemini-tools.sh [backend-suite|suite|mock|catalog|catalog-all|validate|live]\n' >&2; exit 2 ;; esac
if [[ ${GRENGIN_GEMINI_BOUNDED:-0} != 1 ]]; then
  memory=2G
  if [[ "$mode" == backend-suite ]]; then memory=4G; fi
  exec systemd-run --user --quiet --wait --pipe -p "MemoryMax=$memory" -p MemorySwapMax=0 \
    --setenv="PATH=$PATH" --setenv=GRENGIN_GEMINI_BOUNDED=1 \
    --setenv="CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target}" \
    --setenv="GRENGIN_PROVIDER_CATALOG_DIR=${GRENGIN_PROVIDER_CATALOG_DIR:-}" \
    --setenv="GRENGIN_GEMINI_SECRETS_FILE=${GRENGIN_GEMINI_SECRETS_FILE:-}" \
    --setenv="GEMINI_TEST_MODEL=${GEMINI_TEST_MODEL:-gemini-3.1-flash-lite}" \
    --working-directory="$root" bash "$root/tests/providers/gemini-tools.sh" "$mode"
fi
cd "$root"
export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
case "$mode" in
  backend-suite) cargo test --locked --workspace -j 1 -- --test-threads=1 ;;
  suite) cargo test --locked -p llm-plugin -j 1 -- --test-threads=1 ;;
  mock) cargo test --locked -p llm-plugin --test mock_server gemini_ -j 1 -- --test-threads=1 ;;
  catalog)
    : "${GRENGIN_PROVIDER_CATALOG_DIR:?set the catalog ai-providers directory}"
    cargo test --locked -p llm-plugin --test mock_server gemini_catalog_json_schema_tools_complete -j 1 -- --ignored --exact
    ;;
  catalog-all)
    : "${GRENGIN_PROVIDER_CATALOG_DIR:?set the catalog ai-providers directory}"
    cargo test --locked -p llm-plugin --test mock_server every_catalog_manifest_streams_against_the_mock_provider -j 1 -- --ignored --exact
    ;;
  validate)
    : "${GRENGIN_PROVIDER_CATALOG_DIR:?set the catalog ai-providers directory}"
    cargo run --locked -p llm-plugin --example validate_manifests -j 1 -- "$GRENGIN_PROVIDER_CATALOG_DIR"
    ;;
  live)
    : "${GRENGIN_GEMINI_SECRETS_FILE:?set the private shell credentials file path}"
    set -a
    source "$GRENGIN_GEMINI_SECRETS_FILE" >/dev/null 2>&1
    set +a
    : "${GEMINI_API_KEY:?export GEMINI_API_KEY in the private credentials file}"
    cargo test --locked -p llm-plugin --test live_gemini_tools gemini_flash_mcp_json_schema_round_trip -j 1 -- --ignored --exact --nocapture
    ;;
esac
