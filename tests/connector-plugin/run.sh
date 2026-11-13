#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
jobs=${GRENGIN_CARGO_JOBS:-2}
codegen_units=${GRENGIN_TEST_CODEGEN_UNITS:-16}
[[ "$jobs" == 1 || "$jobs" == 2 ]] || { printf 'GRENGIN_CARGO_JOBS must be 1 or 2\n' >&2; exit 2; }
[[ "$codegen_units" =~ ^[1-9][0-9]*$ ]] || { printf 'GRENGIN_TEST_CODEGEN_UNITS must be a positive integer\n' >&2; exit 2; }
http=false
broker=false
oauth_navikt=false
catalog=''
while [[ $# -gt 0 ]]; do
  case "$1" in
    --http) http=true ;;
    --broker) broker=true ;;
    --oauth-navikt) oauth_navikt=true ;;
    --catalog) [[ $# -ge 2 ]] || exit 2; catalog="$2"; shift ;;
    *) printf 'Usage: bash tests/connector-plugin/run.sh [--http] [--broker] [--oauth-navikt] [--catalog DIR]\n' >&2; exit 2 ;;
  esac
  shift
done
cargo fmt --all --check
cargo test --locked --workspace -j "$jobs" \
  --config 'profile.test.incremental=false' \
  --config 'profile.test.package.grengin-api.debug=0' \
  --config "profile.test.package.grengin-api.codegen-units=$codegen_units" -- --test-threads=2
cargo build --locked --workspace -j "$jobs" --config 'profile.dev.package.grengin-api.debug=0' \
  --config "profile.dev.package.grengin-api.codegen-units=$codegen_units"
cargo clippy --locked -p connector-server-plugin --all-targets --no-deps -j "$jobs" -- -D warnings
if [[ -n "$catalog" ]]; then
  GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$catalog" \
    cargo test --locked -p connector-server-plugin --test providers -j "$jobs" -- --ignored --test-threads=2
  cargo run --locked -p connector-server-plugin --example validate_connector_manifests -j "$jobs" -- "$catalog"
fi
if "$http"; then
  bash "$root/tests/connector-plugin/http.sh"
fi
if "$broker"; then
  bash "$root/tests/connector-plugin/broker-regression.sh"
fi
if "$oauth_navikt"; then
  GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="${catalog:-$root/../grengin-list/master-data/connector-plugins}" \
    bash "$root/tests/connector-plugin/oauth-navikt.sh"
fi
git diff --check
