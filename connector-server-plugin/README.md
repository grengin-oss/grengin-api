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
The existing Google/Slack cloud broker is unchanged. Custom JSON cannot select
`grengin_connector_key` or borrow its centrally approved OAuth credentials.
Broker-managed server records cannot be converted to custom plugins while linked
broker connections exist; install a separate server instead.

## Manifest

See [example.json](example.json) for Microsoft Graph mail, drive pagination and
draft creation. It is a configuration example, not a claim of live Microsoft testing.
Use your own Entra application, minimal delegated scopes, and any required admin
consent. This does not use Work IQ or bypass Microsoft licensing/API restrictions.

All 34 provider packages (206 read-only tools) live in the separate `grengin-list` repository,
under `master-data/connector-plugins/<id>/versions/1.0.0/plugin.json`. Each has a
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
the manifest and its read/write tool groups. A pinned setup digest also returns
`installation` and `installation_sha256`, verified against that version's setup.json.
Requests support ETags and never
install catalog entries into the database.

The backend distribution is 0.9.8; the runtime crate is 0.2.0; provider packages
are 1.0.0 with schema_version 1. These versions are independent. Runtime upgrades
do not require rewriting old provider packages. Only missing pinned packages allow
version fallback; invalid hashes and outages fail explicitly. Distributions without
the new family retain working AI/auth discovery and return 404 for connectors.

The local catalog draft is deliberately outside `master-data` with null source
pins until commits are approved. It is not deployable or publishable as-is.

- `schema_version: 1` selects the runtime contract; `version: "1.0.0"` tracks the plugin.
- `auth` is `oauth2`, `api_key` or `none`.
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
- Query values and headers must map to strings, numbers or booleans. Null omits
  a query/header. Use `$omitIfNull` for optional values.
- Bodies and response values reuse `llm-plugin`'s typed mapping language, not
  string substitution or executable scripts. Context roots are `request` (tool
  arguments), `config` (non-secret settings), and `session.response` (vendor JSON).
- `response.error_if` maps a vendor JSON failure flag to a redacted tool error.
- `response.pagination` supports bounded GET next-link pagination with JSON
  Pointers `items_pointer`, `next_pointer`, and `max_pages` (1-10). Next links
  must remain on the original origin and below the base path. Loops, malformed
  next links and exhaustion fail explicitly. Do not combine pagination with `result`.
- JSON-only responses are supported, including an empty 204 as null. Multipart,
  binary downloads, cursor-body pagination and vendor-specific scripting are not
  implemented. Use an existing MCP server for those cases for now.

Do not put secrets in the manifest, headers, configuration or query mappings.
OAuth/API credentials belong in the existing encrypted fields and per-user sessions.
The runtime owns credential injection and never exposes tokens to mappings.

## Security And Resources

The runtime allows HTTPS public destinations by default. Local development needs
both administrator-only `allow_insecure_http` and `allow_private_network` in the
plugin wrapper, outside the manifest. Cloud metadata destinations remain blocked.
DNS is validated by the resolver used for the actual connection; environment
proxies are disabled. Redirects are not followed, including same-origin redirects.
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

## Tests

```bash
bash tests/connector-plugin/run.sh
bash tests/connector-plugin/run.sh --http
bash tests/connector-plugin/run.sh --http --broker
bash tests/connector-plugin/oauth-navikt.sh
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

Generate the JSON Schema with `cargo run -p connector-server-plugin --example schema`.

`--broker` also runs the existing connector OAuth regression suite against a
read-only archive of the website's `feat/connector-open-access` service branch.
It needs that checkout's installed Node dependencies but does not switch branches
or modify/deploy the Worker. Override `GRENGIN_CONNECTOR_SERVICE_REF` when needed.
