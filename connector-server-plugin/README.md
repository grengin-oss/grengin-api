# Connector Server Plugin

A declarative Rust runtime for JSON-defined API tools. This is not a process launcher,
dynamic library loader or a hosted connector platform. HTTP execution is in-process;
the existing MCP tool interface makes these tools usable in Grengin chats.

## Install

Use `POST /admin/mcp-servers` with organization-wide `mcp_servers:admin` permission:

```javascript
const manifest = JSON.parse(pluginFileText);
const server = await request('/admin/mcp-servers', {
  method: 'POST',
  body: JSON.stringify({
    name: manifest.name,
    transport_type: 'http',
    url: manifest.base_url,
    enabled: true,
    default_access: 'explicit_only',
    client_id: '<your vendor OAuth client id>',
    client_secret: '<your vendor OAuth client secret>',
    connection_config: {
      connector_plugin: {manifest, configuration: {}},
      oauth: {
        auth_url: '<vendor authorization URL>',
        token_url: '<vendor token URL>',
        scopes: ['<approved delegated scope>'],
        redirect_url: 'http://localhost:5173/mcp/oauth/callback',
      },
    },
  }),
});
```

The manifest is saved in the existing `mcp_servers.connectionConfig` JSON column.
Its tools and an audit event are saved in the same transaction. No migration,
additional CRUD resource or catalog-wide auto-installation is needed.
An invalid update changes neither the stored configuration nor the running client.
`PUT /admin/mcp-servers/{id}` updates the manifest; delete uses the existing DELETE.
Updates preserve tool IDs, disabled flags and access overrides for matching names.
Removing a tool removes its old metadata. Restart reloads only installed servers.

Users authorize through the existing `POST /mcp/connections/{server_id}/authorize`
and authenticated `GET /mcp/oauth/callback` flow. Tokens remain encrypted with
`APP_KEY`. Chat uses the same `selected_mcp_servers` UUIDs, permissions and audit
trail as other MCP tools. The browser/model never supplies an arbitrary user ID
or another user's connection ID for execution.

This implementation supports administrator-owned OAuth applications, per-user API
credentials (`auth: "api_key"`) and public APIs (`auth: "none"`). For non-OAuth
APIs omit `oauth`, `client_id` and `client_secret`. API-key users call
`POST /mcp/connections/{server_id}/credentials` with
`{"credential":"<private key or token>","expires_at":"<optional RFC3339 expiry>"}`.
The value is encrypted with APP_KEY, never returned, and removed by disconnect.
For a bearer-token-compatible OAuth manifest, an admin can select the explicit
wrapper `auth_mode: "api_key"`, omitting the sibling OAuth object. This supports
Notion internal integration/PAT tokens without pretending they are personal OAuth
sessions. Authentication cannot be disabled using this override; changing the
mode clears prior account credentials. Provider capabilities/page grants still
apply. The selected method is server-wide; each user supplies a separate token.
The existing Google/Slack cloud broker is unchanged. Custom JSON cannot select
`grengin_connector_key` or borrow its centrally approved OAuth credentials.
Broker-managed server records cannot be converted to custom plugins while linked
broker connections exist; install a separate server instead.

## Manifest

See [example.json](example.json) for Microsoft Graph mail, drive pagination and
draft creation. It is a configuration example, not a claim of live Microsoft testing.
Use your own Entra application, minimal delegated scopes, and any required admin
consent. This does not use Work IQ or bypass Microsoft licensing/API restrictions.

The reviewed catalog has 60 provider packages and 521 read-only tools in the separate `grengin-list` repository,
under `master-data/connector-plugins/<id>/versions/<version>/plugin.json`. Each has a
versioned `setup.json` declaring OAuth endpoints/scopes or API credential requirements,
configuration requirements and vendor-specific constraints, without secrets. Their setup
guide is `docs/connector-plugin-providers.md` in that repository. These are locally
mock-tested manifests, not automatically installed or vendor-certified integrations.

## Discovery And Versioning

Grengin API 0.9.8 introduces `GET /discovery/connector-plugins` and
`GET /discovery/connector-plugins/{id}?version=1|1.0.0`, documented in Swagger.
They use the optional `connectorPlugins` family of the exact backend distribution,
sharing the existing AI/auth cache and version selectors. Detail verifies package
SHA-256, identity, version, schema and default destination policy before returning
the manifest, its read/write tool groups and its topic groups. A pinned setup digest also returns
`installation` and `installation_sha256`, verified against that version's setup.json.
Requests support ETags and never
install catalog entries into the database.

The backend distribution is 0.9.8; the runtime crate is 0.6.0; provider packages
use schema_version 1 (55 existing providers gain 1.2.0 packages; five new Google apps start at 1.0.0). These versions are independent. Runtime upgrades
do not require rewriting old provider packages. Only missing pinned packages allow
version fallback; invalid hashes and outages fail explicitly. Distributions without
the new family retain working AI/auth discovery and return 404 for connectors.

The user-approved target remains the early-access 0.9.8 distribution. Its replacement
must pin the reviewed backend and catalog source commits before publication. Older
distributions and previously published versioned packages remain unchanged. Existing
installed manifests do not update automatically when the distribution changes.

## Business Apps

Zoho CRM, Books and Desk have separate US/EU/India installation profiles in the
catalog's setup metadata. An admin selects one profile and its matching regional
OAuth client; there is no automatic callback-directed data-center routing.
Books/Desk organization IDs are validated, per-call arguments. Tokens use the
Zoho-oauthtoken header prefix rather than a Bearer prefix.

Odoo uses the Odoo 19 JSON-2 API with a per-user API key and installation database.
Its explicit allow_custom_public_host policy requires an administrator-provided
override URL and an empty suffix list. It does not disable HTTPS, private-address,
DNS-rebinding or redirect protections. Existing suffix-restricted plugins do not
gain this opt-in automatically.

SAP S/4HANA uses current, fixed OData read paths and customer-provided bearer
credentials. Automatic tenant OAuth acquisition/refresh is not implemented.
Neither these mocks nor reference integrations prove vendor consent, customer
entitlements or full API parity. See the catalog's business-connector-reference-plan.md.

Run the focused 2 GiB, no-swap suite with:

```bash
bash tests/connector-plugin/business.sh
bash tests/connector-plugin/oauth-navikt.sh protocols
```

- `schema_version: 1` selects the runtime contract; `version: "1.0.0"` tracks the plugin.
- `auth` is `oauth2`, `api_key` or `none`.
- Optional `presentation.suggested_prompts` supplies up to eight plain-text
  suggestions (240 characters each), not executable templates or hidden prompts.
- Optional `presentation.topic_groups` (runtime 0.6.0+) groups related tools so an
  administrator can switch them together, for example Gmail's Messages, Threads and
  Attachments. Each group has an `id` (`[a-z][a-z0-9_]*`, up to 64 characters), a
  `name` (up to 60 characters), an optional `description` (up to 240) and its `tools`.
  When groups are present, every tool belongs to exactly one group; at most 32 groups.
  Groups are presentation only: access rules and read-only checks still apply per
  tool. Runtimes before 0.6.0 reject manifests that declare groups.
- `configuration_schema` validates non-secret tenant/account identifiers before installation.
  `base_url_override` belongs in the plugin wrapper; `base_url_policy` can require
  a tenant-specific host and constrain it to approved domain suffixes.
- `credential_delivery` optionally selects a private custom header or JSON body
  field. These values never enter the mapping context. The default is Bearer auth.
- `response.error_array_pointer` rejects GraphQL errors even on HTTP 200.
- Each tool declares an object `input_schema`, read/write annotations and request mapping.
- `required_scopes` documents vendor permission requirements. It does not grant
  permissions or replace Grengin access checks; the provider enforces token scopes.
- Paths append to `base_url`. `{parameter}` must occupy a complete path segment;
  its expression belongs in `path_parameters`. Parameter values cannot contain
  slashes, percent escapes, dot segments, fragments or control characters.
  An explicit array of 1-32 safe segments supports nested paths such as GitHub
  `["src", "lib.rs"]`; each segment is independently checked and encoded.
  Static `resource:action` segments support read-only Google API actions. File
  suffixes such as Zendesk's `.json` use `$stringConcat` inside `path_parameters`.
- Query values accept scalar strings/numbers/booleans or arrays of at most 100
  scalars (repeated query keys, preserving order). Header values remain scalar.
  Null omits a query/header. Use `$omitIfNull` for optional values.
- Bodies and response values reuse `llm-plugin`'s typed mapping language, not
  string substitution or executable scripts. Context roots are `request` (tool
  arguments), `config` (non-secret settings), and `session.response` (vendor JSON).
- `response.error_if` maps a vendor JSON failure flag to a redacted tool error.
- `response.pagination` supports bounded GET next-link pagination with JSON
  Pointers `items_pointer`, `next_pointer`, and `max_pages` (1-10). Next links
  must remain on the original origin and below the base path. Loops, malformed
  next links and exhaustion fail explicitly. Do not combine pagination with `result`.
- Legacy responses remain strict JSON (including an empty 204 as null), with
  1 MiB source/result limits. Opt-in content tools are described below.

## Bounded Content

Runtime 0.5.0 and llm-plugin 0.1.3 add typed content policies:

```json
"response": {
  "format": "text",
  "overflow": "truncate",
  "max_source_bytes": 8388608,
  "max_result_bytes": 65536
}
```

Defaults are `format: "json"` and `overflow: "error"`. Source limits range from
256 bytes to 8 MiB; result limits range from 256 bytes to 1 MiB. Text mode accepts
UTF-8 `text/*` or GitHub's raw media type, not PDF or arbitrary binary downloads.
Text reads stop after a bounded prefix. JSON is parsed completely within its
source cap, then the mapped result is shortened as valid JSON, never byte-sliced.
A source exceeding the JSON cap still fails; filter/project or request a smaller
range. Two content reads may run concurrently across the process.

Opt-in truncation returns:

```json
{"content": "bounded text or JSON", "truncated": false,
 "source_bytes_read": 42, "source_complete": true}
```

`truncated` also covers output projection clipping. `source_complete` describes
whether the source stream was fully consumed, not whether every item in a vendor
collection was paged. Content policy cannot be combined with automatic next-link
pagination. Existing strict tools retain their original result shape.

The shared mapping language adds `$base64Decode` (standard alphabet),
`$base64UrlDecode` (URL-safe alphabet), and `$walk` with a named mapping definition.
Decoders accept padded/unpadded input and MIME line breaks, require decoded UTF-8,
and charge allocation before decoding. An optional sibling `charset` (a label, a
`Content-Type` value or a `[{name, value}]` header list, as in a Gmail MIME part)
decodes legacy charsets instead; with `charset`, malformed bytes become U+FFFD rather
than failing the call, so one odd message part cannot hide a whole thread. Walk visits bounded JSON objects/arrays,
applying a mapping with the current object as `item`; null/omitted results are
skipped. It has step, allocation, node and depth bounds. These operators do not
expose credentials or execute code.

### Downloads, extra hosts and path encodings

Download endpoints often redirect to a pre-signed URL, label every file
`application/octet-stream`, or live on a separate host. Four opt-in manifest fields
cover these without loosening the defaults:

```json
"additional_base_urls": {"content": "https://content.dropboxapi.com/2"},
"tools": [{
  "request": {"method": "GET", "path": "/files/{file_id}/content", "base": "content",
              "path_parameter_encoding": {"file_id": "opaque"}},
  "response": {"format": "text", "overflow": "truncate", "accept_octet_stream": true,
               "redirects": {"allowed_hosts": [".boxcloud.com"], "max_hops": 2}}
}]
```

- `additional_base_urls` (at most 4) are fixed origins in the reviewed manifest that a
  tool selects with `request.base`. Credentials are sent to them; administrator base
  URL overrides never apply to them, and they cannot use next-link pagination.
- `response.redirects` lets a GET follow up to 3 redirects to `allowed_hosts` (exact
  hosts or `.suffix` entries). Every hop must be HTTPS on the default port, passes the
  same DNS/private-network checks, and carries no credentials or tool headers.
  Redirects stay refused for every tool without this policy.
- `response.accept_octet_stream` lets a text tool read `application/octet-stream` or
  untyped bodies; they must still be UTF-8 and contain no NUL bytes, which keeps
  ZIP-based Office files, PDFs and images out.
- A path segment may hold one placeholder among literal text, e.g.
  `A_BusinessPartner('{id}')` or `search(q='{query}')`. Literals are limited to
  letters, digits and `_ - . ( ) ' = ,`. Inside quotes a value's `'` is doubled (the
  OData rule); unquoted values cannot contain quotes, parentheses, `,`, `=` or `:`.
  Values never contain `/`, `\`, `%`, `?` or `#`.
- `request.path_parameter_encoding` marks a whole-segment parameter as `opaque`
  (reserved characters such as `/` are percent-encoded once) or
  `opaque_double_if_slash` (double-encoded when the value starts with `/` or contains
  `//`, as Zoom requires for such meeting UUIDs). `.`/`..` and control characters stay
  refused.

Run `bash tests/connector-plugin/content-audit.sh` for reproducible bounded tests.
The catalog's `docs/connector-content-remediation-2026-11-06.md` records tested
coverage, new scopes, remaining vendor adapters and publication prerequisites.
No plugin is auto-installed, no old package is rewritten and no live vendor
testing or R2 publication is implied by these local mocks.

Do not put secrets in the manifest, headers, configuration or query mappings.
OAuth/API credentials belong in the existing encrypted fields and per-user sessions.
The runtime owns credential injection and never exposes tokens to mappings.

## Connector Detail UI

The signed-in `/mcp-servers` response includes safe plugin identity, selected
authentication method and optional suggested prompts. Organization-wide MCP
viewers can use `/admin/mcp-servers/{id}/overview` and `/people` for the connector
detail screen. MCP admins can change an installed tool's `enabled` flag through
`PUT /admin/mcp-servers/{id}/tools/{tool_id}` without replacing access rules, or
every tool in one topic group through `PUT /admin/mcp-servers/{id}/topic-groups/{group_id}`.
The overview reports each group as `enabled`, `disabled` or `mixed`.
Chat tool results carry bounded previews and execution metadata alongside their
unchanged output. See [the UI contract](../docs/connector-console-api.md) for exact
fields, permission boundaries and unsupported controls. There is no new migration.

## Security And Resources

The runtime allows HTTPS public destinations by default. Local development needs
both administrator-only `allow_insecure_http` and `allow_private_network` in the
plugin wrapper, outside the manifest. Cloud metadata destinations remain blocked.
DNS is validated by the resolver used for the actual connection; environment
proxies are disabled. Redirects are not followed, including same-origin redirects,
unless a GET tool declares a `redirects` allowlist (see Bounded Content); those hops
are HTTPS-only and never carry credentials.
Vendor HTTP error bodies are not returned or logged by this crate.

There are at most 100 tools, a 256 KiB wrapper, 128 KiB arguments/request body,
1 MiB response/result, 4 MiB total downloaded JSON and 16 concurrent calls per
installed connector. Whole calls time out after 30 seconds. Calls over capacity
fail immediately. Dropping a call releases its capacity. Writes are never retried
automatically. The reusable HTTP client has no user token/default credential headers
and no tool-response cache. Each invocation receives the caller's token separately.
URL and custom headers are each limited to 16 KiB. Mapping evaluations are bounded
to 50,000 steps and 4 MiB of estimated allocation per expression, before cloning
or expanding large values; these optional limits do not change LLM mappings.

Changing the plugin URL, transport, OAuth endpoints or client credentials invalidates
the server's saved user tokens atomically. Manifest-only updates on the same binding
preserve user connections. Immediately before each plugin tool invocation, chat
rechecks the user's active status, current server configuration, enabled tool and
MCP permission. Already-dispatched HTTP requests may finish after an admin change;
new tool invocations do not trust permissions captured at the start of the chat.
Explicit tool sync preserves disabled flags and access overrides too.

This targets a single Grengin API instance. No Redis or separate runtime service
is required. Existing stdio MCP servers remain supported, but JSON plugins cannot
execute commands. A sandboxed subprocess adapter is a separate future feature.

## Session Cache

Native plugins do not create an MCP session per user. Their compiled manifest and
credential-free HTTP connection pool are shared; each invocation receives only the
requesting user's credential. Current access and encrypted connection state remain
authoritative in the database. No decrypted-token cache or tool-response cache is
introduced by the remote-session cache below.

Remote authenticated HTTP MCP servers retain sessions with the typed
`SessionCache<K, V>` / `SessionLease<V>` API. The old 256-entry setting is an idle
retention budget, not a user limit. Evicted users reconnect on demand. Configure an
installed remote server through its existing `connection_config`:

```json
{
  "transport_config": {
    "session_cache": {
      "max_entries": 256,
      "idle_ttl_seconds": 300,
      "max_in_flight": 64,
      "wait_timeout_seconds": 30
    }
  }
}
```

Defaults are shown above. Bounds are 0-16384 retained entries, 1-86400 idle
seconds, 1-1024 active operations and 1-120 admission-wait seconds. Zero entries
disables idle retention, not per-user isolation. Unknown cache-policy fields and
out-of-range values are rejected. Native plugin configurations do not accept this
remote-only wrapper.

Each user has a separate session mutex; no global cache lock spans network I/O.
Active or queued requests retain their session even if its idle-cache entry is
evicted. Idle expiry and dead weak-entry cleanup are lazy on subsequent lookups;
this is not a strict total-byte memory cap. Active operations are bounded, while
queued requests time out. Token changes or closed sessions force a new handshake
on the next request. A completed database disconnect invalidates its cached
session; it cannot recall an already-dispatched vendor request. Grengin adds no
automatic tool retry in this cache layer. Sessions necessarily retain their SDK
authentication state; only the extra comparison value is a SHA-256 token digest.

## Tests

```bash
bash tests/connector-plugin/run.sh
bash tests/connector-plugin/run.sh --http
bash tests/connector-plugin/run.sh --http --broker
bash tests/connector-plugin/oauth-navikt.sh
bash tests/connector-plugin/session-audit.sh
GRENGIN_CARGO_JOBS=1 GRENGIN_TEST_CODEGEN_UNITS=64 \
  bash tests/connector-plugin/run.sh --http --broker \
  --catalog ../grengin-list/master-data/connector-plugins
```

The HTTP suite uses a disposable PostgreSQL container, fresh local keys, real
backend routes, a fake OAuth vendor and a fake streaming model. It does not read
your development secrets, touch your database, perform vendor consent, or incur
model charges. Docker and `psql` are needed for `--http` and `--broker`.
`--catalog` runs the explicit cross-repository provider contract tests and the
`validate_connector_manifests` CLI against both aliases and immutable packages.

`oauth-navikt.sh` reuses the pinned `navikt/mock-oauth2-server:6.0.2` used by
auth-config tests, with a disposable database and the already-built backend. It
needs no live credentials or Rust rebuild. `run.sh --oauth-navikt` includes it
after the build gates. Security findings deliberately produce a nonzero exit;
see `docs/connector-plugin-oauth-navikt-review.md` in the backend checkout.

`session-audit.sh` runs the memory-capped core audit: runtime and catalog
contracts, actual backend transport source, 300 concurrent remote-MCP identities,
47 Navikt OAuth setup flows, workspace type checking and focused lint. It requires
the sibling `grengin-list` checkout. The transport target imports the production
OAuth/client/config modules directly, with only the DB transport enum substituted;
it does not cover database authorization or HTTP callback routing. Navikt keeps
vendor-specific grant formats but substitutes mock scopes/endpoints and adapts
vendor response envelopes. It is protocol evidence, not vendor consent evidence.

Core Rust runs at one job with at most 2 GiB/no swap. The Navikt protocol phase splits this into
1536 MiB for Rust and 384 MiB for the disposable mock, then removes that mock.
The user-approved `session-audit.sh full` uses up to 4 GiB for a fresh full backend
build/workspace tests, then 3 GiB for HTTP/full OAuth/broker checks, leaving room
for their disposable databases/mock. It never treats an older binary as validation
of newer source. `console.sh` supports explicit `GRENGIN_CONSOLE_MEMORY_MAX=2G|3G|4G`.
See
`docs/connector-session-cache-audit-2026-11-05.md` for current results and gaps.

Generate the JSON Schema with `cargo run -p connector-server-plugin --example schema`.

`--broker` also runs the existing connector OAuth regression suite against a
read-only archive of the website's `feat/connector-open-access` service branch.
It needs that checkout's installed Node dependencies but does not switch branches
or modify/deploy the Worker. Override `GRENGIN_CONNECTOR_SERVICE_REF` when needed.
