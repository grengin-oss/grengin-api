#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
export GRENGIN_BACKEND_ROOT="$root"
export GRENGIN_GOOGLE_STATE=${GRENGIN_GOOGLE_STATE:-$HOME/.local/state/grengin/google-local}
mkdir -p "$GRENGIN_GOOGLE_STATE"
chmod 700 "$GRENGIN_GOOGLE_STATE"
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}" >/dev/null 2>&1
set +a
: "${GOOGLE_CLIENT_ID:?}" "${GOOGLE_CLIENT_SECRET:?}"
if [[ -f "$GRENGIN_GOOGLE_STATE/runner.pid" ]] && kill -0 "$(< "$GRENGIN_GOOGLE_STATE/runner.pid")" 2>/dev/null; then
  printf 'Google local test is already running.\n'
  exit 1
fi
scratch=$(mktemp -d "${TMPDIR:-/tmp}/grengin-google-local.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT
git -C "$website" archive "${GRENGIN_GOOGLE_WORKER_REF:-feat/connector-open-access}" \
  workers/package.json workers/src/services workers/test/helpers/sqlite-d1.mjs workers/migrations | tar -x -C "$scratch"
ln -s "$website/node_modules" "$scratch/node_modules"
ln -s "$website/workers/node_modules" "$scratch/workers/node_modules"
export GRENGIN_GOOGLE_WORKER_ROOT="$scratch"
if [[ ! -f "$GRENGIN_GOOGLE_STATE/tls.key" ]]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 7 -subj /CN=localhost \
    -addext 'subjectAltName=DNS:localhost,IP:127.0.0.1' \
    -keyout "$GRENGIN_GOOGLE_STATE/tls.key" -out "$GRENGIN_GOOGLE_STATE/tls.crt" >/dev/null 2>&1
fi
cd "$website"
node --import tsx "$root/tests/connectors/google-local.mjs"
