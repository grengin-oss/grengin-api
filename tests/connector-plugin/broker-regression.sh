#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-$root/../grengin-website}
ref=${GRENGIN_CONNECTOR_SERVICE_REF:-feat/connector-open-access}
scratch=$(mktemp -d /tmp/grengin-plugin-broker-source-XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
git -C "$website" archive "$ref" | tar -x -C "$scratch"
ln -s "$website/node_modules" "$scratch/node_modules"
ln -s "$website/workers/node_modules" "$scratch/workers/node_modules"
export GRENGIN_WEBSITE_ROOT="$scratch"
export GRENGIN_CONNECTOR_SECRETS_FILE=/dev/null GRENGIN_CONNECTOR_AUTH_FILE=/dev/null
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}"
export APP_KEY JWT_SECRET
APP_KEY=$(openssl rand -base64 32)
JWT_SECRET=$(openssl rand -hex 32)
printf 'Testing existing broker source at %s (read-only archive)\n' "$(git -C "$website" rev-parse "$ref")"
bash "$root/tests/connectors/run.sh" --live-only
