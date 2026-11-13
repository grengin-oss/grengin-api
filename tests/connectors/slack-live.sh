#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
export GRENGIN_BACKEND_ROOT="$root"
export GRENGIN_WEBSITE_ROOT=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
export GRENGIN_SLACK_STATE=${GRENGIN_SLACK_STATE:-$HOME/.local/state/grengin/slack-consent}
mkdir -p "$GRENGIN_SLACK_STATE"
chmod 700 "$GRENGIN_SLACK_STATE"
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}"
source "${GRENGIN_CONNECTOR_AUTH_FILE:-$root/.grengin_auth_token}"
set +a
: "${SLACK_CLIENT_ID:?}" "${SLACK_CLIENT_SECRET:?}" "${APP_KEY:?}" "${JWT_SECRET:?}"
if [[ ! -f "$GRENGIN_SLACK_STATE/tls.key" ]]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 7 -subj /CN=localhost \
    -addext 'subjectAltName=DNS:localhost,IP:127.0.0.1' \
    -keyout "$GRENGIN_SLACK_STATE/tls.key" -out "$GRENGIN_SLACK_STATE/tls.crt" >/dev/null 2>&1
fi
cd "$GRENGIN_WEBSITE_ROOT"
exec "${GRENGIN_NODE_BIN:-node}" --import tsx "$root/tests/connectors/slack-live.mjs" "$@"
