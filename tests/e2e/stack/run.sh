#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
#
# Full-stack end-to-end run on a Docker host (the staging VM): builds this grengin-api tree,
# prepares a webapp and a catalog checkout, starts a disposable PostgreSQL, then runs the
# Playwright suite inside the official Playwright image with every service on 127.0.0.1.
#
#   WEBAPP_DIR=/path/to/webapp CATALOG_DIR=/path/to/grengin-list bash tests/e2e/stack/run.sh
#
# Without WEBAPP_DIR / CATALOG_DIR it clones WEBAPP_REPO@WEBAPP_REF and CATALOG_REPO@CATALOG_REF.
# Results: $E2E_WORK/report (HTML, JUnit, traces, videos) and $E2E_WORK/logs.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../../.." && pwd)
work=${E2E_WORK:-$HOME/grengin-verify/e2e}
playwright_image=mcr.microsoft.com/playwright:v1.64.0-noble
db_name=grengin_e2e
offset=${E2E_PORT_OFFSET:-0}
db_port=${E2E_DB_PORT:-$((55600 + offset))}
db_container=grengin-e2e-db-$offset-$$
target=${E2E_TARGET_DIR:-$work/target}
mkdir -p "$work"/{logs,report,cargo,home} "$target"

checkout() {
  local dir=$1 url=$2 ref=$3
  if [[ ! -d "$dir/.git" ]]; then git clone --quiet "$url" "$dir"; fi
  git -C "$dir" fetch --quiet origin "$ref"
  git -C "$dir" checkout --quiet --force FETCH_HEAD
}
if [[ -z "${WEBAPP_DIR:-}" ]]; then
  WEBAPP_DIR=$work/webapp
  checkout "$WEBAPP_DIR" "${WEBAPP_REPO:-git@github.com:grengin-oss/webapp.git}" "${WEBAPP_REF:-main}"
fi
if [[ -z "${CATALOG_DIR:-}" ]]; then
  CATALOG_DIR=$work/catalog
  checkout "$CATALOG_DIR" "${CATALOG_REPO:-git@github.com:perter-tech/grengin-list.git}" "${CATALOG_REF:-main}"
fi

if [[ ${E2E_SKIP_BUILD:-0} == 1 && -x $target/debug/grengin-api ]]; then
  echo "== reusing the grengin-api build in $target"
else
echo "== building grengin-api ($(git -C "$repo" rev-parse --short HEAD 2>/dev/null || echo archive))"
docker run --rm --user "$(id -u):$(id -g)" --cpus "${E2E_BUILD_CPUS:-8}" \
  -e CARGO_HOME=/cargo -e CARGO_TARGET_DIR=/target -e HOME=/tmp \
  -v "$repo":/src -v "$work/cargo":/cargo -v "$target":/target -w /src rust:1.91 \
  cargo build --locked -p grengin-api -p migration -j "${E2E_BUILD_CPUS:-8}" > "$work/logs/build.log" 2>&1 \
  || { tail -40 "$work/logs/build.log"; exit 1; }
fi

echo "== starting PostgreSQL on 127.0.0.1:$db_port"
docker run -d --rm --name "$db_container" --network host -e POSTGRES_USER=e2e \
  -e POSTGRES_PASSWORD=e2e-only -e POSTGRES_DB="$db_name" -e PGPORT="$db_port" pgvector/pgvector:pg16 >/dev/null
trap 'docker rm -f "$db_container" >/dev/null 2>&1 || true' EXIT
for _ in $(seq 1 60); do docker exec "$db_container" pg_isready -U e2e -d "$db_name" -p "$db_port" >/dev/null 2>&1 && break; sleep 1; done

echo "== running the suite in $playwright_image"
set +e
docker run --rm --init --network host --ipc host --user "$(id -u):$(id -g)" \
  -e HOME=/home/e2e -e CI=1 \
  -e DATABASE_URL="postgres://e2e:e2e-only@127.0.0.1:$db_port/$db_name" \
  -e E2E_GREP="${E2E_GREP:-}" -e E2E_PORT_OFFSET="$offset" -e E2E_WEBAPP_PREBUILT="${E2E_WEBAPP_PREBUILT:-0}" \
  -v "$work/home":/home/e2e -v "$repo":/repo -v "$target":/target:ro \
  -v "$WEBAPP_DIR":/webapp -v "$CATALOG_DIR":/catalog:ro -v "$work/logs":/logs -v "$work/report":/report \
  -w /repo/tests/e2e "$playwright_image" bash stack/inside.sh
status=$?
set -e
echo "== exit $status; report: $work/report (index.html, junit.xml), logs: $work/logs"
exit $status
