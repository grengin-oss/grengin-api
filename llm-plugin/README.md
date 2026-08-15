# Grengin LLM Provider Plugins

Grengin LLM provider plugins are declarative JSON manifests that translate
Grengin's typed AI requests into a provider's HTTP payloads and translate the
provider's responses back into Grengin events and results. They let a hosted or
self-hosted provider integrate without changing or recompiling Grengin.

The plugin itself is data, not executable code. It cannot run JavaScript,
shell commands, native libraries, or WASM, and it has no direct filesystem or
database access.

## V1 Capabilities

A v1 provider plugin can describe:

- streaming chat over Server-Sent Events (SSE);
- MCP/function tool definitions, streamed calls, results, and continuation;
- provider-native web search and normalized citations;
- text and multimodal message payloads;
- embedding requests and JSON response extraction;
- image generation and editing with JSON, base64, URL, or binary responses;
- static or remote model discovery;
- provider headers, query parameters, paths, and request body encodings;
- token usage, cache-token usage, request IDs, finish reasons, and errors; and
- model capabilities and optional pricing metadata.

Provider communication is restricted to HTTP or HTTPS. Chat responses must use
SSE. Operations may use `GET`, `POST`, `PUT`, or `PATCH`, and request bodies may
be JSON, form data, multipart data, text, binary data, or empty.

## Start A Plugin

For an OpenAI-compatible provider, start from the tested reference:

```bash
cp llm-plugin/examples/openai-compatible.provider.json provider.json
```

The relevant files are:

| File | Purpose |
|---|---|
| [`examples/example.json`](examples/example.json) | Complete, machine-valid v1 reference |
| [`examples/example.annotated.jsonc`](examples/example.annotated.jsonc) | Commented explanation of the same fields |
| [`schema/provider-plugin-v1.schema.json`](schema/provider-plugin-v1.schema.json) | JSON Schema for editor and CI validation |
| [`examples/openai-compatible.provider.json`](examples/openai-compatible.provider.json) | Reusable OpenAI-compatible reference |
| [`examples/anthropic.provider.json`](examples/anthropic.provider.json) | Native Anthropic reference with tools and web search |

JSON does not support comments. Use the JSONC file only as documentation and
submit a strict `.json` manifest to Grengin.

The complete example demonstrates the full crate-level schema, including more
than one credential slot. The current AI-engine API accepts one credential per
custom plugin, so use a single `api_key` slot in an installable v1 manifest.

## Manifest Identity

Every plugin starts with stable identity and compatibility fields:

```json
{
  "$schema": "./provider-plugin-v1.schema.json",
  "manifestVersion": "1.0",
  "id": "example-provider",
  "version": "1.0",
  "name": "Example Provider",
  "description": "Example hosted or self-hosted provider",
  "baseUrl": "https://api.example.com/v1/"
}
```

- `manifestVersion` selects the Grengin host schema. V1 requires `1.0`.
- `id` is the stable AI-engine key. Use lowercase ASCII letters, digits, `-`,
  or `_`, with a maximum of 64 characters.
- `version` tracks the provider plugin release independently from the schema.
  Use `MAJOR.MINOR`; the v1 parser also accepts a numeric patch component for
  existing manifests.
- `baseUrl` must be an absolute HTTP or HTTPS URL without embedded credentials,
  query parameters, or fragments.

Start tested plugins at `1.0`. Increment the minor version for compatible
endpoint, payload, model, pricing, or mapping updates. Increment the major
version when an update changes behaviour in a way that requires administrator
review.

## Credentials And Configuration

A manifest declares credential slots, never credential values:

```json
{
  "credentials": [
    {
      "id": "api_key",
      "type": "secret",
      "required": true,
      "label": "API key"
    }
  ],
  "configurationSchema": {
    "type": "object",
    "properties": {
      "apiVersion": { "type": "string" }
    },
    "required": ["apiVersion"],
    "additionalProperties": false
  }
}
```

The current `ai_engines` integration supports at most one credential slot per
custom plugin. Its value is submitted as `api_key` and encrypted with Grengin's
`APP_KEY`. Do not put keys, tokens, passwords, or private endpoints directly in
a manifest.

Administrator configuration is supplied separately in `plugin_config` and is
validated against `configurationSchema`. JSON Schema defaults document intent,
but Grengin does not automatically insert them.

## Capabilities And Operations

Declare only capabilities the provider implements:

```json
{
  "capabilities": {
    "chat": {
      "streaming": true,
      "tools": true,
      "vision": false,
      "reasoning": false
    },
    "embeddings": true,
    "imageGeneration": false,
    "modelListing": true
  }
}
```

Each enabled capability must have its matching operation. Model listing may
instead use a static `models` array. The main operations are:

| Operation | Purpose |
|---|---|
| `chatStream` | SSE chat, tools, web search, usage, errors, and continuation |
| `embeddings` | Batched text embedding request and vector extraction |
| `imageGeneration` | Prompt-to-image request and response extraction |
| `imageEdit` | Image editing with multipart or another declared body encoding |
| `listModels` | Remote provider model discovery |

Operation paths are relative to `baseUrl`. Dynamic values such as
`${request.model}` are percent-encoded, and normalized paths are checked so
they cannot escape the configured base path.

## Build Provider Payloads

The `mappings` object defines reusable transformations. Operation bodies,
headers, and query parameters can use these bounded operators:

```text
$get          $literal       $omitIfNull   $jsonEncode
$base64       $map           $flatMap      $if
$switch       $merge         $concat       $stringConcat
$coalesce     $object        $array
```

Mappings read only from three typed roots:

- `request`: the canonical chat, embedding, image, or model request;
- `config`: administrator-supplied plugin configuration; and
- `session`: provider state captured for a continuation request.

For example:

```json
{
  "mappings": {
    "message": {
      "role": { "$get": "item.role" },
      "content": { "$get": "item.content" }
    }
  },
  "operations": {
    "chatStream": {
      "method": "POST",
      "path": "chat/completions",
      "headers": {
        "Authorization": { "secret": "api_key", "prefix": "Bearer " }
      },
      "bodyEncoding": "json",
      "body": {
        "model": { "$get": "request.model" },
        "messages": { "$map": "request.messages", "using": "message" },
        "stream": { "$literal": true }
      }
    }
  }
}
```

This fragment only demonstrates request mapping. A working chat operation also
needs the response rules described below.

## Map Chat Streams

`chatStream.response.rules` examines each decoded SSE event and emits canonical
events. Common event kinds are:

```text
messageStart       textDelta          reasoningDelta
toolCallStart      toolArgumentsDelta toolCallEnd
serverToolStart    serverToolQueryDelta
serverToolResult   usage              providerEvent
error              completed
```

A basic text and completion mapping looks like this:

```json
{
  "response": {
    "bodyEncoding": "sse",
    "eventDataEncoding": "json",
    "doneData": "[DONE]",
    "rules": [
      {
        "id": "text",
        "when": { "pointer": "/type", "equals": "content.delta" },
        "emit": "textDelta",
        "value": "/delta"
      },
      {
        "id": "complete",
        "when": { "pointer": "/type", "equals": "message.completed" },
        "emit": "completed",
        "fields": { "finishReason": "/finish_reason" }
      }
    ]
  }
}
```

Tool-capable providers must map a stable call ID, tool name, argument
fragments, and call completion. Configure `continuation` so Grengin can execute
the MCP tool and replay its result. Provider-native search uses the
`serverTool*` events, keeping provider-specific citation data out of the
frontend contract.

The shipped OpenAI, Anthropic, Mistral, and Gemini manifests contain working
examples for their different stream formats.

## Models And Pricing

Use `models` for providers without a model-list endpoint or when Grengin needs
reviewed metadata such as capabilities and pricing:

```json
{
  "models": [
    {
      "id": "example-chat",
      "name": "Example Chat",
      "capabilities": {
        "chat": { "streaming": true, "tools": true }
      },
      "metadata": {
        "inputTokenRate": 0.25,
        "outputTokenRate": 1.0
      }
    }
  ]
}
```

Token rates are USD per million tokens. Cache-read and cache-creation pricing
can be declared separately. Review pricing whenever the provider changes its
published rates.

## Validate A Plugin

Check JSON syntax and validate against the checked-in schema:

```bash
jq empty provider.json
npx --yes ajv-cli@5.0.0 validate \
  --spec=draft2020 \
  --strict=false \
  -s llm-plugin/schema/provider-plugin-v1.schema.json \
  -d provider.json
```

Then run Grengin's semantic validation through a local API. This catches rules
that JSON Schema alone cannot, including capability/operation mismatches,
undeclared credentials, unsafe paths, invalid mappings, and unsupported
manifest versions:

```bash
jq -n --slurpfile manifest provider.json '{
  plugin_config: {
    manifest: $manifest[0],
    configuration: {},
    baseUrlOverride: null,
    allowInsecureHttp: false,
    allowPrivateNetwork: false
  }
}' > validation-request.json

curl --fail-with-body \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary @validation-request.json \
  http://localhost:8080/admin/ai-engines/plugin-validate
```

Validation compiles the mappings but does not contact the provider.

## Install And Test

Create the AI engine disabled first, test the saved credential and endpoint,
and enable it only after the connection succeeds:

```bash
export PROVIDER_API_KEY='replace-with-provider-key'

jq -n --slurpfile manifest provider.json '{
  display_name: $manifest[0].name,
  api_key: env.PROVIDER_API_KEY,
  is_enabled: false,
  whitelisted_models: ($manifest[0].models | map(.id)),
  default_model: ($manifest[0].models[0].id // null),
  plugin_config: {
    manifest: $manifest[0],
    configuration: {},
    baseUrlOverride: null,
    allowInsecureHttp: false,
    allowPrivateNetwork: false
  }
}' > install-request.json

curl --fail-with-body \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary @install-request.json \
  http://localhost:8080/admin/ai-engines

curl --fail-with-body -X POST \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  http://localhost:8080/admin/ai-engines/example-provider/test

curl --fail-with-body -X PUT \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary '{"is_enabled":true}' \
  http://localhost:8080/admin/ai-engines/example-provider
```

Replace `example-provider` with the manifest `id`. If the manifest has no
static `models`, set an explicit whitelist and default model in the install
request or through the admin UI.

## Test Contributions

Run the deterministic plugin and API verification with two Cargo jobs:

```bash
llm-plugin/tests/verify.sh
```

The test layers are:

- unit tests for manifest, mapping, SSE, security, and lifecycle edge cases;
- HTTP runtime tests for embeddings, images, errors, limits, and continuation;
- a local mock provider for chat, tools, search, usage, and failure modes; and
- ignored live tests that require explicit credentials and opt-in environment
  flags.

New provider manifests should include deterministic request and response
fixtures. Live credentials must never be committed or required by the default
test suite.

With `OPENAI_API_KEY`, `GEMINI_API_KEY`, and `MISTRAL_API_KEY` configured, run
the live embedding smoke tests locally with:

```bash
GRENGIN_LIVE_EMBEDDING_TESTS=1 cargo test -p llm-plugin \
  --test live_providers embedding_smoke -j 2 -- --ignored --test-threads=1
```

## Security Rules

- HTTPS is required by default.
- Plain HTTP and private-network destinations require explicit administrator
  opt-in and should be limited to trusted local testing.
- Redirects and normalized operation URLs are checked against the allowed
  destination.
- Authority, forwarding, and other unsafe headers are rejected.
- Manifest, response, timeout, nesting, batch, and tool-round limits are
  enforced by the runtime.
- Operator limits are ceilings; a plugin cannot raise them.
- Provider error bodies, credentials, prompts, and model responses must not be
  written to logs by plugin code.

## Current Limits

- Chat transport is SSE; WebSockets, gRPC, NDJSON, and raw TCP are not
  supported in v1.
- Plugins are JSON manifests, not executable packages.
- The AI-engine API supports one credential slot per custom plugin.
- The active plugin version is exposed as `plugin_version`, but update history,
  rollback, package signing, and automatic remote updates are future work.
- A remote plugin must be reviewed and installed by an administrator; Grengin
  does not automatically execute or activate manifests from a URL.
