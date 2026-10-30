#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}"
set +a
: "${SLACK_CLIENT_ID:?Save SLACK_CLIENT_ID in the secrets file}"
: "${SLACK_CLIENT_SECRET:?Save SLACK_CLIENT_SECRET in the secrets file}"
cd "$website"
node --import tsx "$root/tests/connectors/slack-preflight.mjs"
