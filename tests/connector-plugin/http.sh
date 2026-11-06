#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
scenario=${1:-http}
[[ "$scenario" == http || "$scenario" == oauth-navikt ]] || exit 2
export GRENGIN_BACKEND_ROOT="$root"
export GRENGIN_BACKEND_BINARY="${GRENGIN_BACKEND_BINARY:-${CARGO_TARGET_DIR:-$root/target}/debug/grengin-api}"
[[ -x "$GRENGIN_BACKEND_BINARY" ]] || { printf 'Build the backend first with tests/connector-plugin/run.sh\n' >&2; exit 1; }
sources=$(cd "$root" && rg --files src connector-server-plugin/src -g '*.rs') || exit 1
while IFS= read -r source; do
  if [[ "$root/$source" -nt "$GRENGIN_BACKEND_BINARY" ]]; then
    printf 'Backend binary is older than source; HTTP/OAuth testing refused\n' >&2
    exit 1
  fi
done <<< "$sources"
export GRENGIN_PLUGIN_TEST_ROOT
GRENGIN_PLUGIN_TEST_ROOT=$(mktemp -d /tmp/grengin-plugin-test-XXXXXX)
container="grengin-plugin-test-$$"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf -- "$GRENGIN_PLUGIN_TEST_ROOT"
}
trap cleanup EXIT
password=$(openssl rand -hex 24)
export APP_KEY JWT_SECRET
APP_KEY=$(openssl rand -base64 32)
JWT_SECRET=$(openssl rand -hex 32)
docker run -d --rm --name "$container" --memory=384m --memory-swap=384m --cpus=1 \
  -e POSTGRES_USER=local -e POSTGRES_PASSWORD="$password" -e POSTGRES_DB=grengin_plugin_test \
  -p 127.0.0.1::5432 pgvector/pgvector:pg16 -c shared_buffers=32MB -c max_connections=30 >/dev/null
for _ in $(seq 1 60); do
  if docker exec "$container" pg_isready -U local -d grengin_plugin_test >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp | sed 's/.*://')
export DATABASE_URL="postgres://local:$password@127.0.0.1:$port/grengin_plugin_test"
node "$root/tests/connector-plugin/$scenario.mjs"
