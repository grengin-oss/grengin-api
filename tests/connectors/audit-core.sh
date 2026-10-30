#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
logs=$(mktemp -d /tmp/grengin-connector-core-audit.XXXXXX)
trap 'printf "Core audit evidence: %s\n" "$logs"' EXIT
cd "$root"
git diff --check
bash -n tests/connectors/run.sh tests/connectors/slack-preflight.sh
node --check tests/connectors/http.mjs
printf 'Checking the Worker, broker, and local migration chain...\n'
(
  cd "$website/workers"
  pnpm exec tsc --noEmit
  GRENGIN_CONNECTOR_CATALOG_ROOT="${GRENGIN_CONNECTOR_CATALOG_ROOT:-/home/anurag/work/grengin-list/master-data/connectors}" \
    node --import tsx --test --test-concurrency=2 test/*.test.mjs
  bash test/migrations.sh
) > "$logs/worker.log" 2>&1
printf 'Checking the Rust workspace and disposable HTTP/migration suite (2 jobs, 3 GiB cap)...\n'
systemd-run --user --scope --quiet -p MemoryMax=3G -p MemorySwapMax=0 \
  bash tests/connectors/run.sh > "$logs/backend.log" 2>&1
printf 'Checking the saved Slack settings against published metadata...\n'
bash tests/connectors/slack-preflight.sh > "$logs/slack-preflight.log" 2>&1
printf 'PASS: core automated audit. No shared deployment, remote migration, real vendor consent or GUI testing performed.\n'
