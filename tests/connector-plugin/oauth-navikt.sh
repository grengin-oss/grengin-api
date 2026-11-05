#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
image=${MOCK_OAUTH2_IMAGE:-ghcr.io/navikt/mock-oauth2-server:6.0.2}
container="grengin-plugin-navikt-$$"
cleanup() { docker rm -f "$container" >/dev/null 2>&1 || true; }
trap cleanup EXIT
if ! docker image inspect "$image" >/dev/null 2>&1; then
  config=$(mktemp -d /tmp/grengin-navikt-pull-XXXXXX)
  docker --config "$config" pull "$image"
  rmdir -- "$config"
fi
docker run -d --rm --name "$container" --memory=384m --cpus=1 \
  -p 127.0.0.1::8080 \
  -e 'JAVA_TOOL_OPTIONS=-Xms32m -Xmx160m -XX:MaxMetaspaceSize=96m -XX:ReservedCodeCacheSize=32m' \
  -e 'JSON_CONFIG={"interactiveLogin":false}' "$image" >/dev/null
port=$(docker port "$container" 8080/tcp | sed 's/.*://')
export MOCK_OAUTH2_BASE_URL="http://127.0.0.1:$port"
for _ in $(seq 1 120); do
  if curl --fail --silent --max-time 1 "$MOCK_OAUTH2_BASE_URL/isalive" >/dev/null; then break; fi
  sleep 0.25
done
curl --fail --silent --max-time 2 "$MOCK_OAUTH2_BASE_URL/isalive" >/dev/null
printf 'Connector OAuth review with %s (384 MiB container, no vendor credentials)\n' "$image"
export GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR="${GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR:-$root/../grengin-list/master-data/connector-plugins}"
bash "$root/tests/connector-plugin/http.sh" oauth-navikt
