# Connector Frontend Contract

Backend/service implementation only. The real webapp is not modified or tested
by this patch. All requests below use the normal signed-in API bearer token;
never forward that bearer token to the vendor or Grengin service.

## Start And Confirm

1. POST `/connectors/{catalog_key}/connect` with an exact `version` and optional
   `server_id`. Save the returned connection ID in the signed-in tab.
2. Open its `authorization_url`. It is present only for pending consent.
3. The vendor callback does NOT activate access. It displays a one-time,
   ten-minute confirmation code in that browser's response body.
4. The user returns to their original signed-in account and pastes the code.
   POST `/connectors/{connection_id}/complete`:

```json
{"confirmation_code":"<64 lowercase hex characters>"}
```

5. Render the returned state. Do not show connected merely because the popup
   closed, a callback loaded, or a status says awaiting_confirmation.

The application-level receipt is not OAuth Device Authorization or proof of
the vendor account's email. It stops callback-only activation of a local
connection. It does not make forwarded codes/social engineering impossible.
Do not auto-complete from a URL parameter, accept a connection ID from an
untrusted link, or send the code to an arbitrary opener/origin. A future popup
implementation needs independent owner, window-state and origin validation.

The code is not a provider token and cannot call MCP. Nonetheless treat it as
a transient secret: never log it, put it in URLs, analytics or persistent browser
storage. Polling and listing endpoints intentionally never return it. A lost code
requires disconnect/restart, not a recovery endpoint that reveals the code.

## States

| State | Frontend behavior |
| --- | --- |
| No row | Disconnected; user may start |
| pending | Show consent action; refresh status with backoff |
| awaiting_confirmation | Ask for the browser receipt; no MCP access yet |
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
installation/local user. Honor HTTP 429 Retry-After (bounded 1-300 seconds).
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
