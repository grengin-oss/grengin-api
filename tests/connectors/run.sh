#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
root=$(cd "$(dirname "$0")/../.." && pwd)
website=${GRENGIN_WEBSITE_ROOT:-/home/anurag/work/grengin-website}
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$root/target-security}
cd "$root"
if [[ ${1:-} != --live-only ]]; then
  cargo fmt --all --check
  cargo test --locked --workspace --jobs 2 --config 'profile.test.incremental=false' --config 'profile.test.package.grengin-api.debug=0' --config 'profile.test.package.grengin-api.codegen-units=16' -- --test-threads=2
  cargo build --locked --workspace --jobs 2 --config 'profile.dev.package.grengin-api.debug=0'
fi
set -a
source "${GRENGIN_CONNECTOR_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}"
source "${GRENGIN_CONNECTOR_AUTH_FILE:-$root/.grengin_auth_token}"
set +a
: "${APP_KEY:?APP_KEY must be configured}" "${JWT_SECRET:?JWT_SECRET must be configured}"
export GRENGIN_CONNECTOR_TEST_ROOT
GRENGIN_CONNECTOR_TEST_ROOT=$(mktemp -d /tmp/grengin-connectors-XXXXXX)
container="grengin-connectors-test-$$"
cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf -- "$GRENGIN_CONNECTOR_TEST_ROOT"
}
trap cleanup EXIT
password=$(openssl rand -hex 16)
docker run -d --rm --name "$container" --memory=384m --cpus=1 \
  -e POSTGRES_USER=local -e POSTGRES_PASSWORD="$password" -e POSTGRES_DB=grengin_connectors_test \
  -p 127.0.0.1::5432 pgvector/pgvector:pg16 -c shared_buffers=32MB -c max_connections=30 >/dev/null
for _ in $(seq 1 60); do
  if docker exec "$container" pg_isready -U local -d grengin_connectors_test >/dev/null 2>&1; then break; fi
  sleep 1
done
port=$(docker port "$container" 5432/tcp | sed 's/.*://')
export DATABASE_URL="postgres://local:$password@127.0.0.1:$port/grengin_connectors_test"
export GRENGIN_CONNECTOR_TEST_DATABASE_URL="$DATABASE_URL"
export GRENGIN_BACKEND_BINARY="$CARGO_TARGET_DIR/debug/grengin-api"
export GRENGIN_BACKEND_ROOT="$root"
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost \
  -addext 'subjectAltName=DNS:localhost' \
  -keyout "$GRENGIN_CONNECTOR_TEST_ROOT/key.pem" -out "$GRENGIN_CONNECTOR_TEST_ROOT/cert.pem" >/dev/null 2>&1
cd "$website"
node --import tsx "$root/tests/connectors/http.mjs"
cd "$root"
cargo test --locked --workspace --jobs 2 --config 'profile.test.incremental=false' --config 'profile.test.package.grengin-api.debug=0' --config 'profile.test.package.grengin-api.codegen-units=16' connector_migration_round_trip -- --ignored --test-threads=1
printf 'Connector regressions passed; disposable database and credentials are being removed.\n'
