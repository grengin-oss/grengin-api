#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
# SPDX-License-Identifier: Apache-2.0
#
# Drives the mock provider server with curl, so the wire format can be inspected by hand without
# building anything. Boots the server, runs each scenario, prints the raw SSE, and shuts down.
#
#   ./smoke.sh              # boot a server on a free port and exercise every scenario
#   ./smoke.sh 8080         # use an already-running server on that port

set -euo pipefail
cd "$(dirname "$0")"

own_server=0
if [[ $# -ge 1 ]]; then
  port="$1"
else
  exec 3< <(node provider-server.mjs)
  read -r handshake <&3
  port="$(jq -r .port <<<"$handshake")"
  server_pid="$(pgrep -f -n 'provider-server.mjs')"
  own_server=1
  trap 'kill "$server_pid" 2>/dev/null || true' EXIT
fi

base="http://127.0.0.1:${port}/v1"
echo "mock provider on ${base}"

chat() {
  local route="$1" payload="$2"
  curl -sS --no-buffer -X POST "${base}/${route}" \
    -H 'Content-Type: application/json' \
    -H 'Authorization: Bearer mock-key' \
    -d "$payload"
}

section() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

section 'chat text (OpenAI-compatible)'
chat chat/completions '{
  "model": "mock-model",
  "stream": true,
  "messages": [{"role": "user", "content": "hello there"}]
}'

section 'tool calling (OpenAI-compatible)'
tool_stream=$(chat chat/completions '{
  "model": "mock-model",
  "stream": true,
  "messages": [{"role": "user", "content": "what is the weather in Paris?"}],
  "tools": [{"type": "function", "function": {"name": "mcp__ab12cd34__get_weather__9f3c1d02", "parameters": {}}}]
}')
printf '%s\n' "$tool_stream"
echo "--- reassembled tool arguments ---"
# Concatenate every argument fragment, exactly as the runtime does.
printf '%s\n' "$tool_stream" \
  | sed -n 's/^data: //p' | grep -v '^\[DONE\]$' \
  | jq -r '.choices[0].delta.tool_calls[0].function.arguments // empty' | tr -d '\n'
echo

section 'tool result round trip (OpenAI-compatible)'
chat chat/completions '{
  "model": "mock-model",
  "stream": true,
  "messages": [
    {"role": "user", "content": "what is the weather in Paris?"},
    {"role": "assistant", "content": [], "tool_calls": [{"id": "call_mock_1", "type": "function", "function": {"name": "mcp__ab12cd34__get_weather__9f3c1d02", "arguments": "{\"city\":\"Paris\"}"}}]},
    {"role": "tool", "tool_call_id": "call_mock_1", "content": "{\"content\":[{\"type\":\"text\",\"text\":\"11 degrees\"}],\"isError\":false}"}
  ]
}'

section 'web search citations (OpenAI-compatible)'
chat chat/completions '{
  "model": "mock-model",
  "stream": true,
  "messages": [{"role": "user", "content": "search the web for the rust version"}]
}' | sed -n 's/^data: //p' | grep -v '^\[DONE\]$' \
   | jq -c 'select(.choices[0].delta.annotations) | [.choices[0].delta.annotations[] | {title: .url_citation.title, url: .url_citation.url}]'

section 'web search + client tool in one stream (Anthropic)'
chat messages '{
  "model": "mock-model",
  "max_tokens": 1024,
  "stream": true,
  "messages": [{"role": "user", "content": [{"type": "text", "text": "search for the weather in Paris"}]}],
  "tools": [
    {"type": "web_search_20250305", "name": "web_search"},
    {"name": "mcp__ab12cd34__get_weather__9f3c1d02", "description": "weather", "input_schema": {"type": "object"}}
  ]
}' | sed -n 's/^data: //p' \
   | jq -c 'select(.content_block or .delta) | {type, index, block: .content_block.type, delta: .delta.type}'

section 'error mapping'
for mode in rate-limit payment server; do
  status=$(curl -sS -o /dev/null -w '%{http_code}' -X POST "${base}/chaos/${mode}" -d '{}')
  bytes=$(curl -sS -X POST "${base}/chaos/${mode}" -d '{}' | wc -c)
  printf '  %-12s -> HTTP %s, %s byte body\n' "$mode" "$status" "$bytes"
done

section 'non-streaming operations'
echo -n '  embeddings: '
chat embeddings '{"model": "mock-embed", "input": ["a", "b"]}' | jq -c '[.data[].index]'
echo -n '  images:     '
chat images/generations '{"model": "mock-image", "prompt": "a cat", "n": 2}' | jq -c '[.data[].b64_json]'
echo -n '  models:     '
curl -sS "${base}/models" | jq -c '[.data[].id]'

section 'requests the server observed'
curl -sS "http://127.0.0.1:${port}/__requests" | jq -c '[.[] | {method, path}]'

if [[ $own_server -eq 1 ]]; then
  echo
  echo 'shutting down mock server'
fi
