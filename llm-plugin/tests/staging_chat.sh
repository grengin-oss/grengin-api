#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
AUTH_FILE=${GRENGIN_PROVIDER_AUTH_FILE:-${GRENGIN_STAGING_AUTH_FILE:-"${ROOT_DIR}/.grengin_auth"}}
PROVIDER=${GRENGIN_PROVIDER:-${GRENGIN_STAGING_PROVIDER:-staging-anthropic-plugin}}
MODEL=${GRENGIN_PROVIDER_MODEL:-${GRENGIN_STAGING_MODEL:-claude-haiku-4-5-20251001}}
MCP_SERVER_ID=${GRENGIN_PROVIDER_MCP_SERVER_ID:-${GRENGIN_STAGING_MCP_SERVER_ID:-f7960384-19b7-4c35-a0bb-dc478e731f9f}}
EXPECT_NONZERO_COST=${GRENGIN_PROVIDER_EXPECT_NONZERO_COST:-true}
EXPECT_CACHE_USAGE=${GRENGIN_PROVIDER_EXPECT_CACHE_USAGE:-true}

if [[ -z "${API_URL:-}" || -z "${API_KEY:-}" ]]; then
  if [[ ! -r "$AUTH_FILE" ]]; then
    printf 'Missing API credentials file: %s\n' "$AUTH_FILE" >&2
    exit 2
  fi

  # shellcheck disable=SC1090
  source "$AUTH_FILE"
fi
: "${API_URL:?API_URL must be set by the staging credentials file}"
: "${API_KEY:?API_KEY must be set by the staging credentials file}"

API_URL=${API_URL%/}
AUTH_HEADER="Authorization: Bearer ${API_KEY}"
JSON_HEADER="Content-Type: application/json"
WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/grengin-staging-provider.XXXXXX")
CREATED_CHATS=()
HTTP_STATUS=""
BODY_FILE=""
LAST_CHAT_ID=""

cleanup() {
  local chat_id
  for chat_id in "${CREATED_CHATS[@]}"; do
    curl --silent --show-error --max-time 20 \
      -X DELETE -H "$AUTH_HEADER" \
      "${API_URL}/chat/${chat_id}" >/dev/null || true
  done
  rm -rf "$WORK_DIR"
}
trap cleanup EXIT

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

pass() {
  printf 'PASS: %s\n' "$1"
}

request() {
  local method=$1 path=$2 payload=${3:-}
  BODY_FILE="${WORK_DIR}/response.json"
  if [[ -n "$payload" ]]; then
    HTTP_STATUS=$(curl --silent --show-error --max-time 60 \
      -o "$BODY_FILE" -w '%{http_code}' -X "$method" \
      -H "$AUTH_HEADER" -H "$JSON_HEADER" \
      --data-binary "$payload" "${API_URL}${path}")
  else
    HTTP_STATUS=$(curl --silent --show-error --max-time 60 \
      -o "$BODY_FILE" -w '%{http_code}' -X "$method" \
      -H "$AUTH_HEADER" "${API_URL}${path}")
  fi
}

event_names() {
  sed -n 's/^event:[[:space:]]*//p' "$BODY_FILE" \
    | tr -d '\r"' \
    | sort -u \
    | paste -sd, -
}

has_event() {
  local expected=$1
  sed -n 's/^event:[[:space:]]*//p' "$BODY_FILE" \
    | tr -d '\r"' \
    | grep -Fxq "$expected"
}

remember_conversation() {
  local chat_id
  chat_id=$(sed -n 's/^data:[[:space:]]*//p' "$BODY_FILE" \
    | jq -r 'select(type == "object") | .id // empty' 2>/dev/null \
    | head -1 || true)
  if [[ "$chat_id" =~ ^[0-9a-fA-F-]{36}$ ]]; then
    CREATED_CHATS+=("$chat_id")
    LAST_CHAT_ID=$chat_id
  fi
}

stream() {
  local name=$1 payload=$2
  BODY_FILE="${WORK_DIR}/$(printf '%s' "$name" | tr ' ' '_').sse"
  HTTP_STATUS=$(curl --silent --show-error --no-buffer --max-time 120 \
    -o "$BODY_FILE" -w '%{http_code}' -X POST \
    -H "$AUTH_HEADER" -H "$JSON_HEADER" \
    --data-binary "$payload" "${API_URL}/chat/stream")
  remember_conversation
}

require_successful_stream() {
  local name=$1
  shift
  [[ "$HTTP_STATUS" == "200" ]] || {
    local detail
    detail=$(jq -r '
      if .detail.description then
        "code=" + (.detail.code | tostring) + " " + .detail.description
      else
        .message // "request rejected"
      end
    ' "$BODY_FILE" 2>/dev/null || true)
    fail "${name} returned HTTP ${HTTP_STATUS}: ${detail}"
  }
  has_event ai_error && fail "${name} emitted ai_error (events: $(event_names))"
  local event
  for event in "$@"; do
    has_event "$event" || fail "${name} missed ${event} (events: $(event_names))"
  done
  pass "$name"
}

request GET "/admin/provider-plugins/${PROVIDER}"
[[ "$HTTP_STATUS" == "200" ]] || fail "provider lookup returned HTTP ${HTTP_STATUS}"
jq -e '.status == "enabled"' "$BODY_FILE" >/dev/null \
  || fail "provider ${PROVIDER} is not enabled"
pass "custom provider is installed and enabled"

request POST "/admin/provider-plugins/${PROVIDER}/test" '{}'
[[ "$HTTP_STATUS" == "200" ]] || fail "provider connection test returned HTTP ${HTTP_STATUS}"
jq -e '.valid == true' "$BODY_FILE" >/dev/null \
  || fail "provider connection test did not validate"
pass "stored plugin configuration can be decrypted and compiled"

plain_payload=$(jq -n --arg provider "$PROVIDER" --arg model "$MODEL" '{
  messages: [{content: "Reply with exactly STAGING_PLUGIN_OK", files: [], role: "user"}],
  provider: $provider,
  model_name: $model,
  temperature: 0,
  web_search: false
}')
stream "plain stream" "$plain_payload"
require_successful_stream "plain stream" conversation delta message_end done
plain_text=$(sed -n 's/^data:[[:space:]]*//p' "$BODY_FILE" \
  | jq -r '.text // empty' 2>/dev/null \
  | tr -d '\r\n')
[[ "$plain_text" == *STAGING_PLUGIN_OK* ]] \
  || fail "plain stream did not contain its expected marker"
pass "delta text reached the API client"
message_end=$(sed -n 's/^data:[[:space:]]*//p' "$BODY_FILE" \
  | jq -sc 'map(select(.input_tokens != null and .output_tokens != null and .cost != null)) | last')
jq -e '.input_tokens > 0 and .output_tokens > 0' <<<"$message_end" >/dev/null \
  || fail "plain stream did not report positive input and output token counts"
if [[ "$EXPECT_NONZERO_COST" == "true" ]]; then
  jq -e '.cost > 0' <<<"$message_end" >/dev/null \
    || fail "plain stream reported zero cost for a priced provider model"
fi
pass "stream token counts and cost are populated"

request GET "/chat/${LAST_CHAT_ID}"
[[ "$HTTP_STATUS" == "200" ]] || fail "persisted chat lookup returned HTTP ${HTTP_STATUS}"
jq -e --arg model "$MODEL" '
  .messages
  | map(select(.role == "assistant" and .model == $model))
  | last
  | .usage.input_tokens > 0
    and .usage.output_tokens > 0
    and (.usage.cached_input_tokens | type) == "number"
    and (.usage.cache_creation_tokens | type) == "number"
' "$BODY_FILE" >/dev/null || fail "persisted usage is missing token or cache-token counters"
if [[ "$EXPECT_NONZERO_COST" == "true" ]]; then
  jq -e --arg model "$MODEL" '
    .total_cost > 0
    and (.messages | map(select(.role == "assistant" and .model == $model)) | last | .cost > 0)
  ' "$BODY_FILE" >/dev/null || fail "persisted message or conversation cost is zero"
fi
if [[ "$EXPECT_CACHE_USAGE" == "true" ]]; then
  jq -e --arg model "$MODEL" '
    .messages
    | map(select(.role == "assistant" and .model == $model))
    | last
    | (.usage.cached_input_tokens + .usage.cache_creation_tokens) > 0
  ' "$BODY_FILE" >/dev/null || fail "provider did not report a cache read or cache creation"
fi
pass "persisted token, cache-token, and cost accounting"

error_payload=$(jq -n --arg provider "$PROVIDER" '{
  messages: [{content: "Reply with one word", files: [], role: "user"}],
  provider: $provider,
  model_name: "definitely-not-a-real-model",
  temperature: 0,
  web_search: false
}')
stream "provider error" "$error_payload"
[[ "$HTTP_STATUS" == "200" ]] || fail "provider error case returned HTTP ${HTTP_STATUS}"
has_event ai_error || fail "provider error case did not emit ai_error (events: $(event_names))"
has_event done || fail "provider error case did not terminate with done"
pass "provider error is represented in the SSE contract"

web_payload=$(jq -n --arg provider "$PROVIDER" --arg model "$MODEL" '{
  messages: [{content: "Search the web for the current Rust stable release and cite one source.", files: [], role: "user"}],
  provider: $provider,
  model_name: $model,
  temperature: 0,
  web_search: true
}')
stream "web search" "$web_payload"
require_successful_stream "web search" conversation tool_call tool_result message_end done

mcp_payload=$(jq -n \
  --arg provider "$PROVIDER" \
  --arg model "$MODEL" \
  --arg server "$MCP_SERVER_ID" '{
    messages: [{content: "Call the db_status tool once, then briefly report whether it succeeded.", files: [], role: "user"}],
    provider: $provider,
    model_name: $model,
    temperature: 0,
    web_search: false,
    selected_mcp_servers: [$server],
    selected_tools: ["db_status"]
  }')
stream "MCP db_status" "$mcp_payload"
require_successful_stream "MCP db_status" conversation tool_call tool_result message_end done

printf 'All provider API checks passed. Created conversations will now be removed.\n'
