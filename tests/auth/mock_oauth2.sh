#!/usr/bin/env bash
# Runs credential-free OIDC authorization-code tests against navikt/mock-oauth2-server.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MOCK_OAUTH2_IMAGE="${MOCK_OAUTH2_IMAGE:-ghcr.io/navikt/mock-oauth2-server:6.0.2}"
CONTAINER_NAME="grengin-mock-oauth2-$$"
JSON_CONFIG='{"interactiveLogin":false,"tokenCallbacks":[{"issuerId":"grengin","requestMappings":[{"requestParam":"code","match":"*","claims":{"sub":"mock-user-123","email":"mock.user@example.com","email_verified":true,"name":"Mock User"}}]}]}'

cleanup() {
  docker stop "$CONTAINER_NAME" >/dev/null 2>&1 || true
}
trap cleanup EXIT INT TERM

docker run --detach --rm \
  --name "$CONTAINER_NAME" \
  --publish 127.0.0.1::8080 \
  --env JSON_CONFIG="$JSON_CONFIG" \
  "$MOCK_OAUTH2_IMAGE" >/dev/null

MAPPING="$(docker port "$CONTAINER_NAME" 8080/tcp)"
PORT="${MAPPING##*:}"
BASE_URL="http://127.0.0.1:${PORT}"

for _ in $(seq 1 120); do
  if curl --fail --silent "${BASE_URL}/isalive" >/dev/null; then
    break
  fi
  sleep 0.25
done

curl --fail --silent "${BASE_URL}/isalive" >/dev/null

echo "Testing Grengin OIDC against ${MOCK_OAUTH2_IMAGE} at ${BASE_URL}"
MOCK_OAUTH2_ISSUER="${BASE_URL}/grengin" \
  cargo test --manifest-path "${ROOT_DIR}/Cargo.toml" -p grengin-api -j 2 \
  'auth::mock_oauth2_tests::' -- --ignored --nocapture --test-threads=1
