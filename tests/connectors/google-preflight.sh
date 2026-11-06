#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
ref=${GRENGIN_GOOGLE_WORKER_REF:-feat/connector-open-access}
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}" >/dev/null 2>&1
set +a
: "${GOOGLE_CLIENT_ID:?Save GOOGLE_CLIENT_ID in the secrets file}"
: "${GOOGLE_CLIENT_SECRET:?Save GOOGLE_CLIENT_SECRET in the secrets file}"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/grengin-google-preflight.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
git -C "$website" archive "$ref" workers/package.json workers/src/services workers/test/helpers/sqlite-d1.mjs workers/migrations |
  tar -x -C "$scratch"
ln -s "$website/node_modules" "$scratch/node_modules"
ln -s "$website/workers/node_modules" "$scratch/workers/node_modules"
export GRENGIN_GOOGLE_WORKER_ROOT="$scratch"
printf 'Testing broker source: %s (%s)\n' "$ref" "$(git -C "$website" rev-parse --short "$ref")"
cd "$website"
node --import tsx "$root/tests/connectors/google-preflight.mjs"
