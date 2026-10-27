#!/usr/bin/env bash
set -euo pipefail
set +x
trap 'printf "Test harness failed at line %s.\n" "$LINENO" >&2' ERR
cd "$(dirname "$0")/../.."
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target-security}"
if [[ "${1:-}" != --live-only ]]; then
cargo fmt --all --check
cargo test --locked --workspace --jobs 2
cargo build --locked --workspace --jobs 2
git diff --check
fi

if [[ "${1:-}" != --live && "${1:-}" != --live-only ]]; then
  printf '\nUse --live for isolated PostgreSQL and HTTP regressions (Docker, pg_dump, psql, Node required).\n'
  exit 0
fi

for command in docker pg_dump psql node curl; do command -v "$command" >/dev/null; done
set -a
source "${GRENGIN_SECURITY_SECRETS_FILE:-/home/anurag/work/secrets/grengin.sh}"
set +a
: "${DATABASE_URL:?source database required}" "${JWT_SECRET:?JWT secret required}"
source_database="$DATABASE_URL"
printf 'Preparing disposable PostgreSQL test database...\n'
scratch="$(mktemp -d /tmp/grengin-security-XXXXXX)"
container="grengin-security-$(basename "$scratch" | tr '[:upper:]' '[:lower:]')"
server_pid=''
cleanup() {
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; fi
  docker rm -f "$container" >/dev/null 2>&1 || true
  rm -rf -- "$scratch"
}
trap cleanup EXIT
docker run -d --rm --name "$container" -e POSTGRES_USER=local \
  -e POSTGRES_PASSWORD=isolated-security-test-only -e POSTGRES_DB=grengin_security_test \
  -p 127.0.0.1::5432 pgvector/pgvector:pg16 >/dev/null
for _ in $(seq 1 60); do
  if docker exec "$container" pg_isready -U local -d grengin_security_test >/dev/null 2>&1; then break; fi
  sleep 1
done
pg_dump --no-owner --no-acl "$source_database" | \
  docker exec -i "$container" psql -X -q -v ON_ERROR_STOP=1 -U local -d grengin_security_test >"$scratch/restore.log" 2>&1
db_port="$(docker port "$container" 5432/tcp | sed 's/.*://')"
export DATABASE_URL="postgres://local:isolated-security-test-only@127.0.0.1:$db_port/grengin_security_test"
export DB_URL="$DATABASE_URL"
# Copied MCP configuration may point at real databases/services or spawn commands.
# The dedicated MCP DB regressions use rows directly, not those external clients.
psql -X -q -v ON_ERROR_STOP=1 "$DATABASE_URL" -c 'UPDATE mcp_servers SET enabled = false' >/dev/null
if [[ "${GRENGIN_ENGINE_INSTALL_TESTS:-0}" == 1 ]]; then
  psql -X -q -v ON_ERROR_STOP=1 "$DATABASE_URL" -c 'DELETE FROM ai_engines' >/dev/null
fi
export HOST=127.0.0.1
export PORT="${GRENGIN_SECURITY_TEST_PORT:-18080}"
export FILE_STORAGE_ROOT="$scratch/files"
export RAG_ENABLED=false
export GRENGIN_AUTO_MIGRATE=true
export GRENGIN_SECURITY_TEST_URL="http://$HOST:$PORT"
if curl --max-time 2 -fsS "$GRENGIN_SECURITY_TEST_URL/" >/dev/null 2>&1; then
  printf 'Test port is occupied; choose GRENGIN_SECURITY_TEST_PORT.\n' >&2; exit 1
fi
start_backend() {
"$CARGO_TARGET_DIR/debug/grengin-api" >"$scratch/server.log" 2>&1 &
server_pid=$!
ready=false
for _ in $(seq 1 120); do
  if curl --max-time 2 -fsS "$GRENGIN_SECURITY_TEST_URL/" >/dev/null 2>&1; then ready=true; break; fi
  if ! kill -0 "$server_pid" 2>/dev/null; then printf 'Test backend failed to start.\n' >&2; exit 1; fi
  sleep 1
done
[[ "$ready" == true ]] || { printf 'Test backend readiness timed out.\n' >&2; exit 1; }
}
start_backend
if [[ "${GRENGIN_CATEGORY_TESTS:-0}" == 1 ]]; then
  node tests/discovery/categories-http.mjs
fi
if [[ "${GRENGIN_ENGINE_INSTALL_TESTS:-0}" == 1 ]]; then
  node tests/ai-engines/http.mjs
  engine_count="$(psql -X -A -t "$DATABASE_URL" -c 'SELECT count(*) FROM ai_engines')"
  kill "$server_pid"; wait "$server_pid" 2>/dev/null || true; server_pid=''
  start_backend
  [[ "$(psql -X -A -t "$DATABASE_URL" -c 'SELECT count(*) FROM ai_engines')" == "$engine_count" ]]
  [[ "$(psql -X -A -t "$DATABASE_URL" -c "SELECT count(*) FROM ai_engines WHERE \"engineKey\"='groq'")" == 0 ]]
  printf 'PASS: restart preserves installed rows and never reinstalls a deleted catalog engine.\n'
fi
node tests/security/http.mjs
if [[ "${GRENGIN_EXTENSIVE_TESTS:-0}" == 1 ]]; then
  node tests/security/extended-http.mjs
fi
if [[ "${GRENGIN_SHARE_CHAT_TESTS:-0}" == 1 ]]; then
  node tests/share-chat/http.mjs
fi
cargo test --locked --workspace --jobs 2 refresh_cannot_undo_disconnect_reauthorization_or_deletion -- --ignored --test-threads=1
docker exec "$container" createdb -U local grengin_upload_test
GRENGIN_UPLOAD_TEST_DATABASE_URL="${DATABASE_URL%/*}/grengin_upload_test" \
  cargo test --locked --workspace --jobs 2 concurrent_uploads_keep_distinct_rows_and_share_bytes -- --ignored --test-threads=1
cargo test --locked -p sqlx-mcp --jobs 2 -- --ignored --test-threads=1
printf '\nSecurity regressions passed; disposable server and database will be removed.\n'
