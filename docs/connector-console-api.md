# Connector Detail UI Contract

Prepared for backend 0.9.8, on `feat/connector-server-plugin`. This follow-up adds
exact execution pagination, a permission-filtered picker with batched access
resolution, and atomic topic-group switches. Rebuild this branch before staging
these changes. No frontend
page, database migration or shared-service deployment is included.

## Screen Mapping

| Design element | API / behavior |
| --- | --- |
| Installed connector identity | `GET /mcp-servers` now includes `integration.plugin_id`, `plugin_version` and `authentication`; never join by display name |
| My account status | `/mcp-servers` includes typed `connection_state`: connected, not_connected, needs_reconnect or unavailable |
| Administration detail page | `GET /admin/mcp-servers/{server_id}/overview` |
| Native package catalog | `GET /discovery/connector-plugins`, detail `GET /discovery/connector-plugins/{id}?version=1` |
| Connected by 6 of 9 people | `connected_user_count` / `active_user_count`; local stored state, active workspace accounts, not eligibility or a vendor health probe |
| Connected-count widget | Use `overview.connected_user_count`; there is no separate count endpoint |
| Turn off for everyone | Existing `PUT /admin/mcp-servers/{server_id}` with `{"enabled": false}`; retain account tokens, block future tool execution |
| Connect my account | Select flow using `integration.authentication`, described below |
| Try asking | `integration.suggested_prompts`; versioned optional manifest presentation, empty for providers without curated prompts; hide the section when empty |
| What Grengin can do | `overview.tools`: UUID, description, `is_read_only`, `enabled`, access inheritance; only installed tools are supported |
| Tool switch | `PUT /admin/mcp-servers/{server_id}/tools/{tool_id}` with `{"enabled": false}` or `true` |
| Tool groups | `overview.topic_groups`: `id`, `name`, optional `description`, `state` (`enabled`, `disabled`, `mixed`) and `tool_ids` into `overview.tools` |
| Group switch | `PUT /admin/mcp-servers/{server_id}/topic-groups/{group_id}` with `{"enabled": false}` or `true` |
| Who can use it | `GET/PUT /admin/mcp-servers/{server_id}/access` and `overview.access`; use `all_users`, `admin_only`, or `explicit_only` plus department rules |
| People | `GET /admin/mcp-servers/{server_id}/people?limit=20&offset=0`; limit 1-100, exact offset, deterministic ordering |
| This week | `overview.usage`, last rolling seven days: actual tool calls, successful calls, failed calls |
| Open audit log | `GET /admin/mcp-servers/{server_id}/executions?limit=20&offset=0`, exact row offsets, optional `tool_name`, `is_error`, `user_id` filters |
| Advanced | Existing server configuration, tool access and execution endpoints |

The console and people endpoints require organization-wide `mcp_servers:view`;
tool switches require organization-wide `mcp_servers:admin`. Ordinary users never
receive the people roster through `/mcp-servers`. Tokens, client secrets, OAuth
authorization URLs and vendor payloads are absent from the console response.
Successful console/people responses use `Cache-Control: private, no-store`.
The people query selects only profile/status fields, never encrypted tokens.

Counts deliberately use **active workspace accounts**, including accounts which
may currently be excluded by a connector access policy. Do not label this count
"eligible users". Disabling a server retains stored connection counts while its
displayed per-user state becomes `unavailable`.

Overview returns 401 for missing sign-in, 403 without organization-wide viewing
permission, and 404 for an unknown server. Expired tokens with refresh
credentials count as connected; revoked broker connections, empty credentials
and inactive users do not. It never refreshes a token or calls a vendor.

Execution history uses a stable `executed_at DESC, id DESC` order for timestamp
ties. The existing limit is clamped to 1-100 (default 50); offsets over 1,000,000
return 400. Unknown servers return 404, not an empty successful list. This is
offset pagination, not a frozen snapshot: new executions can shift later pages.

`GET /mcp-servers` now filters installed servers and tools using the existing chat
access rules. Admin-only/explicit-only defaults hide denied servers; explicit
server and tool grants remain honored. Only permitted enabled tools are exposed.
It still returns no account tokens or workspace roster.

The picker and tool list share one resolution pass: one user access context,
batched server/tool rules, one lookup of referenced department paths, and one
resolution per server. Explicit per-tool grants remain supported. Query-count
regressions compare one versus thirteen department-scoped connectors.

## Topic Groups

The 55 existing connectors gain 1.2.0 packages, and five new Google apps have
1.0.0 packages with groups. They group related tools by topic, such as Gmail's
Messages, Threads, Attachments, Drafts and Labels, so a long tool list can be switched
a group at a time. Groups come from the installed plugin version's manifest; a
server installed from 1.1.0 or earlier, or a custom MCP server, returns an empty
`topic_groups` list, so render the flat `tools` list instead. When groups are present,
every installed tool belongs to exactly one group.

`state` is derived from the installed tools: `enabled` when all are on, `disabled`
when all are off, `mixed` otherwise. Individual tool switches keep working and can
make a group mixed. The group switch requires organization-wide `mcp_servers:admin`.
It resolves membership on the server from the installed manifest, never from the
request, and updates every tool plus one `mcp.tool_group_enabled_changed` audit event
in one transaction. It returns `{"group": ..., "tools": [...]}` with the new state and
the updated tools. Unknown servers or group IDs return 404.

Groups do not change access: a disabled group only turns its tools off, and enabling
one does not override access rules or read-only permissions. The read-only versus
read/write classification stays separate (`tool_groups` in discovery,
`is_read_only` per tool). Discovery detail also returns the package's `topic_groups`
with tool names, for showing groups before installation.

The frontend mock's `getConnectedUsersCount()` helper must be removed/replaced,
not implemented as a backend compatibility route. The admin list also does not
return `connected_users_count`: do not infer zero or call overview once for every
row. Render the measured count on the detail page from its existing overview.

For the existing access UI, change just the default with
`PUT /admin/mcp-servers/{server_id}/access/default`. Add a rule with
`POST .../access/rules`; it returns **one McpAccessRule**, not a full access list.
Delete it with `DELETE .../access/rules/{rule_id}` (204), then refetch `/access`.
`/{server_id}/tools/access` manages bulk **tool** policies, not server policies.
The legacy frontend org-wide `/connection` and `/connection/disconnect` URLs are
not implemented here. Use per-user `/mcp/connections/{server_id}/...` instead;
do not share one person's OAuth session with the organization.

## Authentication

`integration` is safe to expose in the signed-in server listing:

```json
{
  "plugin_id": "notion",
  "plugin_version": "1.2.0",
  "suggested_prompts": ["Find pages about the project roadmap."],
  "authentication": "api_token",
  "supported_authentication": ["oauth2", "api_token"],
  "per_user_credentials": true
}
```

Methods are `oauth2`, `api_token`, `grengin_broker`, `server_managed` and
`unavailable`. The selected method is server configuration, not an end user's
arbitrary choice. Render the proper flow; don't infer it from the name, icon,
HTTP transport or an untrusted provider URL. Continue to use the effective-access
API to decide whether a user may connect or select that server.

- `oauth2`: existing `POST /mcp/connections/{server_id}/authorize`, followed by the
  existing authenticated `/mcp/oauth/callback`. Preserve the catalog's complete
  OAuth metadata: JSON/form encoding, PKCE, token selectors and refresh behavior.
- `api_token`: existing `POST /mcp/connections/{server_id}/credentials`, body
  `{"credential":"<user supplied token>"}` with optional future `expires_at`.
  Keep it in a password input, never URLs/localStorage/logs, and clear the input
  after saving. Tokens remain per-user and encrypted using `APP_KEY`.
- `grengin_broker`: existing `/connectors/{key}/connect`, status, complete and
  disconnect APIs. This is the legacy, trusted hosted connector binding.
- `server_managed`: no per-user OAuth/token button; server administration and
  effective tool permissions still apply.
- `unavailable`: configuration is invalid; disable Connect and show an admin-only
  configuration action. Do not fall back to another protocol or shared identity.

For an OAuth manifest, an admin may explicitly install it using
`connection_config.connector_plugin.auth_mode = "api_key"`, removing the sibling
`connection_config.oauth` object. This selects a manually supplied bearer/API token
using the manifest's existing credential delivery. It does not invent a different
provider authentication protocol. The runtime refuses an override to `none`, or
an API-key/public manifest being promoted to OAuth. Changing auth mode clears
prior tokens and pending OAuth states atomically. Ordinary users cannot edit it.

### Notion

Notion supports OAuth, internal integration tokens and personal access tokens.
Its integration capabilities and page grants are separate from Grengin's access
policy. An internal integration token acts as that integration and sees pages
shared with it; it is not guaranteed to act as the signed-in Grengin user. Do not
show the Gmail-style "only your personal inbox" assurance for arbitrary tokens.
OAuth consent may require selecting pages. A successful token save does not prove
every page is accessible; vendor calls can still return permission errors.

Source: [Notion authorization](https://developers.notion.com/guides/authorization/overview).

The target early-access distribution is
`https://meta.grengin.com/distributions/grengin-api/0.9.8/index.json`. Its previously
published revision pins backend `fc13154` and 55 native plugins at 1.1.0. The user
approved keeping 0.9.8 rather than creating 0.9.9: the replacement will pin the
reviewed runtime 0.6.0 and 60 plugins (521 tools). Publication is a separate step;
check the index source commits before assuming the replacement is live. Existing
installed manifests require an explicit admin update, not just a backend restart.
Use backend discovery, not global aliases. Older distributions and published
package bytes remain unchanged.

## Tool Results

The browser receives a summary of each connector call and result, never its
arguments or output. The model still receives the full result, and the admin
execution log (`mcp_executions`) keeps it. This applies to the live stream and to
chat history; replies saved before this change are summarised when they are read.
Both fail closed. In the live stream only the provider's own server-side web
search is web search; every function call is a connector's, including names
that match no connector or more than one, which are summarised as `Connector`.
In history every stored call and result except the providers' own web search
is summarised, including calls that never got a result, whatever alias of the
tool name they used. A stored entry counts as web search only by its exact tool
name (`web_search`, `web_search_call`, `google_search`, ...) and only when
neither it nor its result carries a connector marker (`mcp` or `summary`), so a
connector named like web search is still summarised.

A connector `tool_call` has no `input` or `input_text`; it carries:

```json
{
  "tool_name": "tool_0_search_threads",
  "tool_id": "call_1",
  "kind": "other",
  "summary": { "connector": "Gmail", "action": "Search threads", "target": "from:acme invoice" }
}
```

`target` is optional and is taken only from a search, name, path, channel or
recipient argument, clipped to 80 characters. Bodies, message text and IDs are
never shown. A connector call is sent exactly once, when its arguments are
complete; no partial chunks are sent for it.

A connector `tool_result` has no `output`; it carries `mcp` (execution details)
and `summary`:

```json
{
  "tool_id": "call_1",
  "status": "success",
  "kind": "other",
  "mcp": {
    "execution_id": "96dbd42d-1a45-4e26-a217-dba38ad52c66",
    "server_id": "8c2fa647-4a8e-4ac1-9f38-93df735621c0",
    "server_name": "Gmail",
    "tool_name": "search_threads",
    "duration_ms": 140,
    "executed_at": "2026-11-05T08:00:00Z"
  },
  "summary": { "text": "2 messages", "item_count": 2, "items": ["Invoice #123", "Re: Payment"] }
}
```

`summary.text` is always present: a count (`2 messages`, `No results`), the
title of a single item (an empty list beside a title, such as an issue without
labels, still shows the title), `Done` when the result has no title, or one of these
fixed error messages: `Sign-in required`, `Sign-in failed`,
`Connector unavailable, try again`, `Access denied`, `Connector not connected`,
`Timed out`, `Did not finish`, `The tool returned an error`. Error text from
the connector or vendor is never forwarded, since it can quote a message body
or a credential; it stays in the execution log.
`item_count` and `items` appear only for lists; `items` holds at most five
titles (email subject, file or page name, event or issue summary), each clipped
to 120 characters. Snippets and bodies are never included. Render text as text,
not raw HTML. Web search events are unchanged and omit `mcp` and `summary`.

`execution_id` correlates with the existing execution audit when its best-effort
write succeeds; this change does not make execution auditing transactional with
external vendor side effects. No public audit access or credential logging is
added. Do not send private tool payloads to browser console logs or analytics.

## Honest Capability Boundaries

- All current first-party native packages remain read-only. Do not show working
  draft/send/delete switches for tools which do not exist. A read-only policy
  still overrides an enabled write tool on a custom manifest.
- `write_confirmation_supported` is false. The screenshot's per-action approval
  promise is not implemented by a tool-enable switch. Existing custom write tools
  retain their current execution behavior; this work does not add consent gates.
- `reminders_supported` is false. Hide "Remind the others" for now; no emails,
  notifications or silent notification fan-out are added.
- Do not label tool calls as questions answered, or failed calls as PII blocks.
  Draft and PII metrics require their own measured event definitions.
- Native plugins still use instance-owned OAuth clients. Shared native cloud
  OAuth broker integration remains separate API/service work; preloading Worker
  credentials alone does not enable it.
- This work does not change the frontend. Consume the additive fields to render
  the design. Refetch private connection state after OAuth, token save, disconnect
  and administrative changes. Scope browser query keys by API base and user.

## Verification Commands

The backend-only HTTP suite starts disposable fixtures, not the user's demo
backend or real vendor sessions. Use two compile jobs:

```bash
bash tests/connector-plugin/review.sh
```

The HTTP suite uses a fresh disposable database and local vendor/model fixtures,
not the user's development database or real connector accounts. No paid model or
external write calls are needed. The existing live test services are left alone.

`tests/connector-plugin/console-contract.mjs` is invoked by `http.sh`. It checks
the actual frontend URLs, sign-in and permission failures, counts and privacy,
policy changes reflected in the picker, cross-server tool isolation, exact
execution offsets, timestamp ties, filter combinations and malformed queries.
Swagger and pure pagination regression cases are also cargo tests.
