# Connector Frontend Contract

All requests below use the normal signed-in API bearer token;
never forward that bearer token to the vendor or Grengin service.
The local frontend integration is in `work/fresh/webapp`; it is not published
with the backend/service changes. Deploy a compatible webapp alongside them.

## Start And Confirm

1. POST `/connectors/{catalog_key}/connect` with an exact `version` and optional
   `server_id`. Save the returned connection ID, authorization URL's `state`,
   signed-in user ID and ten-minute expiry in the initiating browser session.
2. Open its `authorization_url`. It is present only for pending consent. For a
   popup, copy this context into its same-origin sessionStorage before navigating
   to the vendor. Do not depend on an unvalidated `postMessage` or opener.
   For registration-free connections this URL is the Grengin service's destination
   approval page, not the vendor: it shows the connector, its permissions and this
   instance's domain, and the user must choose Continue before the vendor opens.
   The approval is bound to that browser, so open the URL in the same browser
   profile that will finish the flow (a popup or new tab is fine). The URL still
   carries `state`. Starting MCP authorization for a native server that is already
   connected or awaiting confirmation returns 409 rather than a URL.
3. The vendor still redirects only to the shared service callback. grengin-api
   signs the instance return URL derived from REDIRECT_URL, so the operator does
   not register every instance URL with Slack. Public instances require HTTPS;
   plain HTTP is allowed only for localhost/loopback testing.
4. The broker does NOT activate access from that anonymous callback. It redirects
   to `<REDIRECT_URL>/mcp/oauth/callback` with a fragment containing
   `connector_approval`, `connection_id` and the original `state`. Remove the
   fragment immediately, before asynchronous work. Refuse missing, duplicated or
   unexpected fields, mismatched state/connection/user, absent browser context
   and expired attempts. Only then automatically POST to the signed-in API:

```json
{"confirmation_code":"<64 lowercase hex characters>"}
```

5. Verify connected status with GET, clear the stored transaction, then render
   success. Do not show connected merely because the popup
   closed, a callback loaded, or a status says awaiting_confirmation.

The application-level receipt is not OAuth Device Authorization or proof of
the vendor account's email. It stops callback-only activation of a local
connection. It does not make forwarded codes/social engineering impossible.
Do not auto-complete from URL parameters alone, accept a connection ID from an
untrusted link, or send the code to an arbitrary opener/origin. The returned
receipt is usable only after independent owner and browser-transaction checks.

The code is not a provider token and cannot call MCP. Nonetheless treat it as
a transient secret: never log it, put it in query strings, analytics or persistent
browser storage. The return fragment is browser-only and must be scrubbed at once.
Polling and listing endpoints intentionally never return it. A lost code
requires disconnect/restart, not a recovery endpoint that reveals the code.

Old backend starts without a signed instance `redirect_uri` retain the manual
code page for compatibility. Existing pending attempts must be disconnected and
restarted after upgrading; their return destination cannot be changed in flight.
No vendor token, backend bearer token, private key or OAuth authorization code
is delivered to the instance callback.

## States

| State | Frontend behavior |
| --- | --- |
| No row | Disconnected; user may start |
| pending | Show consent action; refresh status with backoff |
| awaiting_confirmation | Complete only from the verified browser return; no MCP access yet |
| connected | Consent confirmed; actual tool access still follows existing MCP policies |
| expired | Restart after disconnect; finish cleanup first if revocation_pending |
| revoked | Disabled; retry disconnect if revocation_pending |
| failed | Show reconnect action after disconnect |

`GET /connectors` is a stored list, not current vendor introspection. Use
`GET /connectors/{id}` when checking broker state. `connected_at` is the first
local verification of connected status, not an exact vendor event timestamp.

The scope list is requested catalog metadata, not an introspected vendor grant.
No browser response includes vendor access/refresh tokens or a broker grant.

## Retry And Disconnect

Use explicit status refresh/backoff; do not poll tightly. Broker quotas are per
connection. Honor HTTP 429 Retry-After (bounded 1-300 seconds).
Starts also have a local pre-write limit of 20 per minute and at most 64 stored
connections per user. A capacity conflict requires disconnecting unused rows,
not repeatedly retrying. Connector JSON requests are limited to 16 KiB.
An upstream proof error returns 403, not 401: it must not sign the local user out.

Error bodies retain the application's rich error envelope. `external_code` is
a fixed safe connector reason: connector_not_found, connector_forbidden,
connector_reconnect_required, connector_rate_limited, connector_broker_disabled,
invalid_connector_request, connector_capacity_exceeded or connector_service_unavailable. Unknown upstream
errors become a sanitized 503; no vendor error body is copied to the UI.

If completion returns 503, GET status before starting a new flow. The broker may
have committed approval while the local save failed; status reconciliation can
finish the local save without replaying the confirmation code.

POST `/connectors/{id}/disconnect` has these important outcomes:

- 200 with revocation_pending=false: local row removed; GET now returns 404.
- 200 with revocation_pending=true: disabled, with encrypted credentials retained
  only for cleanup. Retry the same disconnect with backoff.
- 503: do not claim success. GET status to distinguish a local DB failure from
  local revocation followed by a broker outage, then retry.

`provider_revocation` can be revoked, pending, unsupported or not_applicable.
Unsupported means broker credentials were erased, NOT that the vendor account's
OAuth consent was revoked. Offer the vendor's account settings/manual revoke path.

Existing MCP connection listings and disconnect remain integrated. MCP runtime
errors keep their older API/SSE shape; the new connector HTTP error contract does
not change every legacy MCP endpoint's error representation.

## API Paths

```text
GET  /connectors
POST /connectors/{connector_key}/connect
GET  /connectors/{connection_id}
POST /connectors/{connection_id}/complete
POST /connectors/{connection_id}/disconnect
```

Catalog entries are not installed automatically. Executing tools still needs an
accessible, enabled admin-configured HTTP MCP server at the pinned catalog URL.
Use its OAuth configuration and map additional catalog MCP headers into the
existing transport_config.headers field, not OAuth credentials. Only streamable
HTTP is broker-supported in this patch; listing an SSE/stdio transport does not
enable those execution paths.
