#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
#
# Runs inside the Playwright image (see run.sh): starts the mocks, the catalog, grengin-api
# and the webapp on 127.0.0.1, then runs the Playwright suite.
set -euo pipefail

# E2E_PORT_OFFSET lets several stacks share one host (each parallel run uses its own offset).
o=${E2E_PORT_OFFSET:-0}
export OIDC_PORT=$((9401 + o)) LLM_PORT=$((9402 + o)) CATALOG_PORT=$((9403 + o)) MCP_PORT=$((9404 + o))
export API_PORT=$((18200 + o)) WEB_PORT=$((4173 + o))
export E2E_API_URL=http://127.0.0.1:$API_PORT E2E_WEB_URL=http://127.0.0.1:$WEB_PORT
export CATALOG_DIR=/catalog/master-data
export JWT_SECRET=${JWT_SECRET:-$(head -c 32 /dev/urandom | base64 | tr -d '=+/' )}
export APP_KEY=${APP_KEY:-$(head -c 32 /dev/urandom | base64)}

pids=()
cleanup() { for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null || true; done; }
trap cleanup EXIT
start() { local name=$1; shift; "$@" > "/logs/$name.log" 2>&1 & pids+=($!); }

start oidc node mocks/oidc.mjs
start llm node mocks/llm.mjs
start catalog node mocks/static.mjs
# Further mock services: any mocks/service-*.mjs is started with the same environment.
for mock in mocks/service-*.mjs; do
  [[ -e $mock ]] || continue
  name=$(basename "$mock" .mjs)
  start "$name" node "$mock"
done

start api env \
  HOST=127.0.0.1 PORT=$API_PORT DATABASE_URL="$DATABASE_URL" \
  APP_KEY="$APP_KEY" JWT_SECRET="$JWT_SECRET" GRENGIN_AUTO_MIGRATE=true \
  FILE_STORAGE_ROOT=/home/e2e/files RAG_ENABLED=false \
  GRENGIN_METADATA_BASE_URL=http://127.0.0.1:$CATALOG_PORT/ \
  BOOTSTRAP_SUPER_ADMIN_EMAIL=owner@e2e.grengin.test \
  REDIRECT_URL=$E2E_WEB_URL GOOGLE_CLIENT_ID=e2e-unused GOOGLE_CLIENT_SECRET=e2e-unused \
  /target/debug/grengin-api

cd /webapp
if [[ ${E2E_WEBAPP_PREBUILT:-0} == 1 && -f dist/index.html && -d node_modules ]]; then
  echo "== reusing the built webapp"
  export PATH=/home/e2e/bin:$PATH
else
echo "== building the webapp"
mkdir -p /home/e2e/bin
export PATH=/home/e2e/bin:$PATH COREPACK_ENABLE_DOWNLOAD_PROMPT=0
corepack enable --install-directory /home/e2e/bin
# The webapp pins no packageManager; its lockfile (v9) matches the pnpm its developers use.
corepack prepare "pnpm@${E2E_PNPM_VERSION:-10.17.0}" --activate > /logs/corepack.log 2>&1
pnpm install --frozen-lockfile > /logs/webapp-install.log 2>&1 || { tail -30 /logs/webapp-install.log; exit 1; }
env -u VITE_API_BASE pnpm run build > /logs/webapp-build.log 2>&1 || { tail -30 /logs/webapp-build.log; exit 1; }
fi
start web env VITE_API_BASE=$E2E_API_URL ./node_modules/.bin/vite preview --host 127.0.0.1 --port $WEB_PORT --strictPort

cd /repo/tests/e2e
npm install --no-audit --no-fund > /logs/e2e-install.log 2>&1
set +e
npx playwright test ${E2E_GREP:+--grep "$E2E_GREP"}
status=$?
set -e
cp -r report/. /report/ 2>/dev/null || true
cp -r test-results /report/test-results 2>/dev/null || true
exit $status
