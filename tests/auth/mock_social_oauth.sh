#!/usr/bin/env bash
# Runs credential-free LinkedIn and Apple OIDC tests against navikt/mock-oauth2-server.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
MOCK_OAUTH2_IMAGE="${MOCK_OAUTH2_IMAGE:-ghcr.io/navikt/mock-oauth2-server:6.0.2}"
CONTAINER_NAME="grengin-social-mock-oauth2-$$"
JSON_CONFIG='{"interactiveLogin":false,"tokenCallbacks":[{"issuerId":"linkedin","requestMappings":[{"requestParam":"code","match":"*","claims":{"sub":"linkedin-user-123","email":"linkedin.user@example.com","email_verified":true,"name":"LinkedIn User","picture":"https://cdn.example.com/linkedin-user.png"}}]},{"issuerId":"apple","requestMappings":[{"requestParam":"code","match":"*","claims":{"sub":"apple-user-123","email":"apple.user@privaterelay.appleid.com","email_verified":true,"name":"Apple User","is_private_email":true}}]}]}'

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

echo "Testing LinkedIn and Apple OIDC against ${MOCK_OAUTH2_IMAGE} at ${BASE_URL}"
MOCK_OAUTH2_BASE_URL="${BASE_URL}" \
  cargo test --locked --manifest-path "${ROOT_DIR}/Cargo.toml" -p grengin-api --jobs 2 \
  'auth::social_mock_tests::' -- --ignored --nocapture --test-threads=1
