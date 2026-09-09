#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

# Exercises the public provider-discovery API against an already running server.

set -euo pipefail

base_url="${GRENGIN_API_URL:-http://127.0.0.1:8080}"
work_dir=$(mktemp -d)

cleanup() {
  find "$work_dir" -type f -delete
  rmdir "$work_dir"
}
trap cleanup EXIT INT TERM

request_status() {
  local output_file="$1"
  local url="$2"
  shift 2
  curl --silent --show-error --output "$output_file" --write-out '%{http_code}' "$@" "$url"
}

status=$(request_status "$work_dir/root.json" "$base_url/")
test "$status" = 200
jq -e '.status == "Okay"' "$work_dir/root.json" >/dev/null

for kind in auth-providers ai-providers; do
  status=$(request_status \
    "$work_dir/$kind-latest.json" \
    "$base_url/discovery/$kind")
  test "$status" = 200
  jq -e '.providers | length > 0' "$work_dir/$kind-latest.json" >/dev/null

  headers="$work_dir/$kind.headers"
  list="$work_dir/$kind.json"
  status=$(curl --silent --show-error \
    --dump-header "$headers" \
    --output "$list" \
    --write-out '%{http_code}' \
    "$base_url/discovery/$kind?version=1")
  test "$status" = 200
  jq -e '
    .catalog_type
    and (.providers | length > 0)
    and all(.providers[];
      has("selected_version")
      and has("available_versions"))
  ' "$list" >/dev/null

  while IFS=$'\t' read -r id version; do
    major_detail="$work_dir/$kind-$id-major.json"
    exact_detail="$work_dir/$kind-$id-exact.json"

    status=$(request_status \
      "$major_detail" \
      "$base_url/discovery/$kind/$id?version=1")
    test "$status" = 200
    jq -e --arg id "$id" --arg version "$version" '
      .id == $id
      and .version == $version
      and (.sha256 | test("^[0-9a-f]{64}$"))
    ' "$major_detail" >/dev/null

    status=$(request_status \
      "$exact_detail" \
      "$base_url/discovery/$kind/$id?version=$version")
    test "$status" = 200
    cmp --silent "$major_detail" "$exact_detail"
  done < <(jq -r '.providers[] | [.id, .selected_version] | @tsv' "$list")
done

jq -e '.template | has("schemaVersion") and has("configuration")' \
  "$work_dir/auth-providers-apple-major.json" >/dev/null
jq -e '.plugin | has("manifestVersion") and has("operations")' \
  "$work_dir/ai-providers-openai-major.json" >/dev/null

etag=$(awk '
  BEGIN { IGNORECASE = 1 }
  /^etag:/ { sub(/\r$/, "", $2); print $2 }
' "$work_dir/auth-providers.headers")
test -n "$etag"
status=$(request_status \
  "$work_dir/not-modified.body" \
  "$base_url/discovery/auth-providers?version=1" \
  --header "If-None-Match: $etag")
test "$status" = 304
test ! -s "$work_dir/not-modified.body"

test "$(request_status "$work_dir/invalid-version.json" \
  "$base_url/discovery/auth-providers?version=bogus")" = 400
test "$(request_status "$work_dir/unknown-provider.json" \
  "$base_url/discovery/auth-providers/unknown?version=1")" = 404
test "$(request_status "$work_dir/unsupported-major.json" \
  "$base_url/discovery/auth-providers/apple?version=9")" = 404
test "$(request_status "$work_dir/invalid-provider.json" \
  "$base_url/discovery/auth-providers/Apple?version=1")" = 404

curl --silent --show-error "$base_url/openapi.json" > "$work_dir/openapi.json"
for path in \
  '/discovery/auth-providers' \
  '/discovery/auth-providers/{provider}' \
  '/discovery/ai-providers' \
  '/discovery/ai-providers/{provider}'; do
  jq -e --arg path "$path" '.paths[$path].get' "$work_dir/openapi.json" >/dev/null
done

printf 'Provider discovery API end-to-end checks passed against %s\n' "$base_url"
