<!--
SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
SPDX-License-Identifier: Apache-2.0
-->

# llm-plugin — handoff notes

State of the declarative provider-plugin runtime after a bug-hunt, a web-search implementation, and
an error-handling review. Written for whoever picks this up next.

`llm-plugin` turns a JSON manifest into a working LLM provider: SSE chat, embeddings, image
generation and model listing, with request payloads and response events described declaratively so a
new provider needs no Rust.

**Branch:** `feat/llm-plugin` · **Tests:** 90 plugin + 4 API-boundary offline, 13 live
(`#[ignore]`d) · clippy clean

---

## 1. How to run things

```bash
# Offline suite — no network, no node required for most of it
cargo test -p llm-plugin -j 2

# Regenerate the checked-in JSON schema after ANY manifest type change.
# `tests/manifests.rs::checked_in_json_schema_is_current` fails until you do.
cargo run -p llm-plugin --example generate_schema -j 2

# Mock provider server, by hand (curl + jq, no build)
llm-plugin/tests/mock/smoke.sh          # boots its own server
llm-plugin/tests/mock/smoke.sh 8080     # target a running one

# Live providers. NOTE: fish is the default shell here, so wrap the source in bash.
bash -c 'set -a; source /home/anurag/work/secrets/grengin.sh; set +a
         export GRENGIN_LIVE_PROVIDER_TESTS=1
         cargo test -p llm-plugin --test live_tooling -j 2 -- --ignored --nocapture'
```

### Test layout

| Target | Count | What it covers |
|---|---|---|
| `src/**` unit tests | 47 | mapping evaluation, SSE decode/map, manifest validation, security |
| `tests/runtime_http.rs` | 29 | runtime against hand-written frames from an in-process Rust server |
| `tests/mock_server.rs` | 10 | **shipped manifests** against the Node mock provider |
| `tests/manifests.rs` | 4 | reference and complete examples parse; checked-in schema is current |
| `tests/live_providers.rs` | 8 | chat smoke per provider (ignored) |
| `tests/live_tooling.rs` | 5 | MCP tool round trip + web search, real providers (ignored) |

`runtime_http.rs` tests the runtime; `mock_server.rs` tests the manifests we ship. Keep both — the
usage bug in §2.14 was only caught by the latter.

`examples/example.json` is the complete, machine-valid manifest reference. JSON cannot contain
comments, so `examples/example.annotated.jsonc` explains the same fields and alternatives for
authors; submit the `.json` file, never the `.jsonc` guide.

---

## 2. Bugs fixed

Each was reproduced by a test that fails without the fix.

### 2.1 Array literals silently became operator expressions
serde derives struct-from-sequence for `untagged` enum variants, so a bare JSON array matched an
operator struct by arity before reaching `ArrayValue`:

| mapping fragment | parsed as |
|---|---|
| `[]` | array ✓ |
| `[x]` | `$literal` ✗ |
| `["request.messages", "message"]` | `$map` ✗ |
| `[a, b]` / `[a, b, c]` | `$if` ✗ |
| 4+ elements | array ✓ |

So `"stop": ["\n\n", "END"]` became a `$map`. Fixed by making `ArrayValue` the **first** variant of
`MappingExpression`. **The variant order is now load-bearing** — there is a doc comment on the enum
and `reads_bare_arrays_of_every_length_as_array_literals` locks it. Do not reorder.

### 2.2 Every OpenAI tool-call stream aborted
OpenAI sends `"content": null` alongside tool calls. The manifest's `exists: true` guard matched the
null, then string extraction failed and killed the stream. Reference manifest now uses `notNull`.

### 2.3 Rate limits misreported as size errors
`ensure_success` did `read_limited(...).await?`, so a 429 with a body over the 8 KB cap returned
`ResponseTooLarge` instead of `QuotaExhausted` — callers never backed off. The drain result is now
discarded so the status always wins.

### 2.4 Long chat streams truncated
reqwest's `RequestBuilder::timeout` is a *total* deadline including body read, so any answer longer
than `timeoutMs` (default 120 s) was cut mid-stream. There are now two clients: `client` (buffered
ops, total deadline) and `stream_client` (`connect_timeout` + `read_timeout`, i.e. inactivity only).
`RequestMode` selects between them.

### 2.5 Path traversal out of the base URL
The manifest check rejected `..` but the URL standard also resolves `%2e%2e`, and a caller-supplied
model id in a path template could be `..`. Two layers now: `is_dot_dot_segment` rejects every
encoding at manifest validation, and `ensure_within_base` re-checks the **normalised** path prefix at
request time. The runtime check is the one that matters — encoding tricks all normalise away first.

### 2.6 Duplicate completion events
A `finish_reason` rule *and* `doneData` both fired. The mapper now seals — see §2.14 for the
important qualification.

### 2.7 Model ids mangled in path templates
`NON_ALPHANUMERIC` encoded the unreserved set, so `gpt-4.1-mini` went out as `gpt%2D4%2E1%2Dmini`.
`PATH_VALUE` preserves `-._~`. This is safe *only* because §2.5 checks containment after
normalisation.

### 2.8 Silent auth misconfiguration
`HeaderValueSpec`'s inline untagged variants accepted a misspelled `prefix`, sending a bare
credential instead of `Bearer <key>`. Variants now wrap named structs (`SecretHeaderValue` etc.) so
each can `deny_unknown_fields` — serde only allows that attribute on containers, not variants.

### 2.9 Assistant narration dropped on tool continuation
The replayed assistant turn had `content: Vec::new()`, losing text the model emitted before calling a
tool. Anthropic rejects such a turn outright. `ObservedToolCalls` now accumulates `TextDelta`.

### 2.10 Repeated tool ids hard-errored
Several OpenAI-compatible vendors echo the tool id in every fragment. Re-announcing the same id at
the same index is now a no-op; a *different* id at a taken index is still an error.

### 2.11 Static model lists were unusable
`modelListing: true` with a `models` array and no `listModels` operation failed validation. A static
list now satisfies the capability.

### 2.12 Whole-float token counts killed streams
`"input_tokens": 12.0` failed `as_u64`. `value_to_u32` accepts integers, whole floats and numeric
strings, and is shared between `sse.rs` and `runtime.rs`.

### 2.13 Unbounded image downloads
Generated-image URL fetches had no timeout and ignored the operation's response cap. `count: 0` also
wasted a request before failing.

### 2.14 Usage dropped after completion (regression, caught by the mock server)
The §2.6 sealing discarded **everything** after the finish-reason chunk. But OpenAI sends
`stream_options: {include_usage: true}` totals in a chunk *after* it, and the shipped manifest sets
that flag — so token counts, which `src/utils/chat_stream.rs` bills on, were silently lost.

Sealing is now selective via `survives_completion`: content and tool events after completion are
dropped as trailing noise; `usage`, `error` and `providerEvent` still apply. **If you touch
completion handling, keep this distinction.**

### 2.15 Web search bypassed the typed request boundary
The first implementation required browser-controlled `ChatInput.config.nativeTools` to contain an
Anthropic-native tool object. That bypassed the product's `web_search` switch, leaked provider
protocol into the client, and could allow other provider-side tools through a shipped manifest.
`ChatRequest.web_search` is now canonical. The Anthropic manifest owns its
`web_search_20250305` declaration and conditionally adds it when the typed flag is true.

Providers also omit server-tool IDs in some citation formats. `PluginStreamParser` now assigns a
stable stream-local ID, including for result-only events, so the main chat lifecycle does not drop
those citations. Streamed query JSON is reassembled into the structured web-search state before the
final result is persisted. Conflicting client/server assignments in one provider index now fail
closed instead of aliasing two tools.

---

## 3. Web search — implemented

Three event kinds, mapped declaratively and wired through to the API layer:

| `ProviderEvent` | `StreamParseResult` (src/handlers/llm) |
|---|---|
| `ServerToolStart { id, name, query, queries }` | `WebSearchAction` |
| `ServerToolQueryDelta { id, name, fragment }` | `ToolInput` (caller concatenates, as it already does for client tools) |
| `ServerToolResult { id, name, results }` | `WebSearchResult` |

`ServerToolResultItem` mirrors `StreamWebSearchResult` field for field. Queries live on
`ServerToolStart`; a provider reporting queries *and* citations together (Gemini grounding metadata)
is mapped with one rule of each kind on the same event.

Three mapping additions made it expressible:

- **`collect` + `itemFields`** — gathers an array into *one* event with per-item field mapping. This
  is what keeps provider internals out: only mapped fields survive, so Anthropic's ~1.2 KB
  `encrypted_content` per citation never reaches the caller. Live payload went from ~30 KB to ~3 KB.
- **`$concat`** — flattens arrays, so `tools` can carry mapped MCP client tools *plus* a
  manifest-owned native web-search declaration selected by `request.webSearch`. Web search and MCP
  in one request works without browser-supplied provider payloads.
- **`$coalesce`** — first non-null wins. Anthropic's `max_tokens` is mandatory while
  `ChatRequest.maxTokens` is optional; without this the whole payload failed.

### The hard part: shared index spaces

Anthropic numbers server tools and client tools in **one** `content_block` index space and streams
both inputs as an identical `input_json_delta`. The discriminator (`content_block.type`) only appears
in the earlier `content_block_start`, and a `when` condition can only see the event in front of it.

`MapperState` therefore tracks `tool_ids_by_index` and `server_tools_by_index` separately. A rule
whose index belongs to the *other* space resolves to `Resolved::OtherSpace` and is skipped; an index
in neither is still a hard error. This is what lets one `toolArgumentsDelta` rule and one
`serverToolQueryDelta` rule coexist on the same `when` condition.

Verified live on Anthropic: 8 citations, query reassembles to
`{"query":"current stable Rust compiler version"}`, MCP client tool called and its result round-tripped
in the same session.

---

## 4. Error handling for custom provider configs

- **Operator limits are ceilings, not defaults.** Every site used
  `spec.max_response_bytes.unwrap_or(runtime.max_response_bytes)`, so a manifest declaring 64 MB beat
  an operator configured for 512 bytes. `timeout_for` / `response_limit_for` now clamp with `.min()`.
  Relevant because manifests are operator-supplied but not necessarily operator-audited.
- **`configurationSchema` compiles during manifest validation**, so a broken schema is reported at
  submit time rather than on first provider use.
- **`ProviderError::is_retryable()` / `is_configuration_fault()`.** Retrying a `MissingCredential` or
  `PayloadMapping` burns quota and buries the mistake. 4xx (except 408/409/425/429) and all
  mapping/manifest/URL faults are non-retryable; transport, `StreamEnded`, 5xx and `QuotaExhausted`
  are.
- **Bounded error text.** `$switch has no case for …` interpolated whatever the expression produced —
  potentially message content in logs. Now truncated to 48 chars and it names the declared cases.
- **Credentials are checked for header-safety at construction**, naming the slot. A line-wrapped API
  key used to surface as an opaque per-request `HeaderNotAllowed`.

Error messages were checked for secret leakage: provider response bodies, credential values and
mapped payloads stay out of `ProviderError`. `runtime_http.rs` asserts this for 401/402/429/503.

---

## 5. Mock provider server

`tests/mock/provider-server.mjs` — zero-dependency Node, speaks both dialects. Prints
`{"port":N}` on stdout line 1, then serves.

| Route | Behaviour |
|---|---|
| `POST /v1/chat/completions` | OpenAI-compatible SSE |
| `POST /v1/messages` | Anthropic content-block SSE |
| `POST /v1/embeddings`, `/v1/images/generations`, `GET /v1/models` | buffered ops |
| `POST /v1/chaos/{rate-limit,payment,server,not-sse,truncated}` | failure modes |
| `GET /__requests`, `POST /__reset` | what the plugin actually sent |

Scenario comes from the last user message — "weather" → tool call, "search" → web search, otherwise
plain text; a tool result in history triggers the follow-up turn.

It deliberately reproduces quirks that have broken this runtime: `content: null` beside tool calls, a
tool id repeated in every fragment, JSON split mid-token across writes, CRLF frames, `: keepalive`
comments, `[DONE]` after a finish reason, usage in a trailing `choices: []` chunk, embeddings in
reverse index order, 1.2 KB of `encrypted_content` per citation, and a 45 KB 429 body. **When adding
a scenario, add the quirk too** — that is where the value is.

---

## 6. Performance cleanups

- `$map` no longer deep-clones the whole request per element. `Scope { root, item, definitions }` is
  threaded through `evaluate`; `MappingContext`'s public API is unchanged.
- `SseEventMapper` no longer clones every rule per event — spec and mutable state are separate fields
  so one event borrows the rules.
- Deduped the SSE push/finish loop and the two u32 parsers; simplified the `notNull` predicate and
  the `MessageStart` filter/collect trick.

---

## 7. Open items for the next iteration

Roughly by value:

1. **Reasoning/thinking replay.** `ReasoningDelta` is never captured into the replayed assistant
   turn. Anthropic requires thinking blocks replayed verbatim *with signatures* for tool use, so
   extended thinking + tools will not work. Needs a domain change, not just a mapping one.
2. **`tool_choice` is unmapped.** It exists on `ChatRequest` but neither reference manifest maps it,
   so a tool cannot be forced. `ToolChoice::Named` also serializes as `{"named": "..."}`, which is
   awkward to map — consider flattening it.
3. **Anthropic `system` role.** Still mapped into `messages`; Anthropic wants a top-level `system`
   parameter. Pre-existing, not introduced here. Needs either a manifest-level "extract role" concept
   or a dedicated field.
4. **`StructuredBodyEncoding::TextJson`** is accepted by the schema but decoded identically to
   `Json`. There is a `TODO` on `decode_structured`. Either give it distinct behaviour or drop the
   variant before manifests rely on it — a product decision, deliberately left alone.
5. **Validation errors carry no JSON path.** A 200-line manifest reports
   `JSON pointer must be empty or start with '/'` with no location. `serde` errors do include
   line/column; the semantic checks do not. Threading a path through `validate_*` would help
   operators a lot.
6. **`jsonschema` validator is recompiled per `DeclarativeProvider::new`.** Fine if providers are
   cached in the registry; wasteful if constructed per request. Worth checking how `state.rs` uses it.
7. **DNS-rebinding TOCTOU** remains between `validate_destination` and reqwest's own resolution.
   Closing it needs a custom resolver.
8. **Mock server keys scenarios on prompt substrings**, so a prompt containing "search" or "weather"
   silently changes behaviour. An explicit `x-mock-scenario` header would be more robust.

### Behaviour change to be aware of

`notNull: false` now also matches an **absent** pointer, not only an explicit `null`. This was an
untested edge before; the new reading is consistent with `exists: false`.

---

## 8. Environment gotchas

- **`OPENAI_API_KEY` in `/home/anurag/work/secrets/grengin.sh` is wrapped across 4 lines** (3 embedded
  newlines). The runtime correctly refuses it — a credential with a line break is a header-injection
  vector. Put the value on one line; the key itself is valid (verified 200 via curl with newlines
  stripped). This is the only "failure" in the live suite that is actually actionable.
- DeepSeek and Cerebras return **402** — no credit on those accounts.
- `GEMINI_API_KEY` has an `AQ.Ab8R` prefix, not a standard `AIza…` key, and the OpenAI-compat endpoint
  returns 404. Fixture or credential issue, not a runtime one.
- The default shell here is **fish**, so `source secrets.sh` fails. Use
  `bash -c 'set -a; source …; set +a; …'`.
- Live results at time of writing: `live_tooling` 5/5 pass; `live_providers` 4/8 (Groq, Mistral,
  Anthropic, OpenRouter pass).
