#!/usr/bin/env bash
set -euo pipefail
set +x
root=$(cd "$(dirname "$0")/../.." && pwd)
mode=${1:-check}
memory=${GRENGIN_CONSOLE_MEMORY_MAX:-2G}
case "$memory" in 2G|3G|4G) ;; *) printf 'Memory cap must be 2G, 3G or 4G\n' >&2; exit 2 ;; esac
case "$mode" in check|runtime|catalog|lint|unit|build|http|oauth|broker|local-compatibility) ;; *) printf 'Usage: bash tests/connector-plugin/console.sh [check|runtime|catalog|lint|unit|build|http|oauth|broker|local-compatibility]\n' >&2; exit 2 ;; esac
if [[ ${GRENGIN_CONSOLE_BOUNDED:-0} != 1 ]]; then
  exec systemd-run --user --quiet --wait --pipe \
    -p "MemoryMax=$memory" -p MemorySwapMax=0 \
    --setenv=GRENGIN_CONSOLE_BOUNDED=1 \
    --setenv="PATH=$PATH" \
    --working-directory="$root" bash "$root/tests/connector-plugin/console.sh" "$mode"
fi
cd "$root"
export CARGO_BUILD_JOBS=1
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_DEBUG=0
case "$mode" in
  check) cargo check --locked --workspace --all-targets -j 1 ;;
  runtime) cargo test --locked -p connector-server-plugin -j 1 -- --test-threads=1 ;;
  lint) cargo clippy --locked --workspace --all-targets --no-deps -j 1 ;;
  local-compatibility)
    node --input-type=module <<'NODE'
const deadline = Date.now() + 360000;
while (Date.now() < deadline) {
  const results = [];
  for (const id of ['gmail', 'notion']) {
    try {
      const response = await fetch(`http://127.0.0.1:8083/discovery/connector-plugins/${id}?version=1`, {signal: AbortSignal.timeout(5000)});
      const body = await response.json();
      results.push({id, status: response.status, version: body.version});
    } catch { results.push({id, status: 0}); }
  }
  if (results.every(result => result.status === 200 && result.version === '1.0.0')) {
    console.log(JSON.stringify({existing_backend_catalog_compatibility: results}));
    process.exit(0);
  }
  await new Promise(resolve => setTimeout(resolve, 5000));
}
throw new Error('Existing backend did not recover baseline catalog compatibility');
NODE
    ;;
  catalog)
    export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="$root/../grengin-list/master-data/connector-plugins"
    cargo test --locked -p connector-server-plugin --test providers -j 1 -- --ignored --test-threads=1
    ;;
  unit)
    cargo test --locked --workspace -j 1 \
      --config 'profile.test.incremental=false' \
      --config 'profile.test.package.grengin-api.debug=0' \
      --config 'profile.test.package.grengin-api.codegen-units=256' -- --test-threads=1
    ;;
  build)
    cargo build --locked -p grengin-api -j 1 \
      --config 'profile.dev.package.grengin-api.debug=0' \
      --config 'profile.dev.package.grengin-api.codegen-units=256'
    ;;
  oauth) bash tests/connector-plugin/oauth-navikt.sh integration ;;
  broker)
    GRENGIN_CARGO_JOBS=1 GRENGIN_TEST_CODEGEN_UNITS=256 \
      bash tests/connector-plugin/broker-regression.sh
    ;;
  http)
    binary=${GRENGIN_BACKEND_BINARY:-${CARGO_TARGET_DIR:-$root/target}/debug/grengin-api}
    [[ -x "$binary" ]] || { printf 'Build the patched backend before HTTP testing\n' >&2; exit 1; }
    sources=$(rg --files src connector-server-plugin/src -g '*.rs') || {
      printf 'Cannot enumerate patched sources; HTTP testing refused\n' >&2
      exit 1
    }
    while IFS= read -r source; do
      if [[ "$source" -nt "$binary" ]]; then
        printf 'Backend binary is older than the source; refusing a misleading HTTP run\n' >&2
        exit 1
      fi
    done <<< "$sources"
    bash tests/connector-plugin/http.sh
    ;;
esac
