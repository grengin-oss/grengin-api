# Tools popup: working and needs-reconnect connectors (future plan)

Status: not started. Blocked by ENGG-545 (design). Recorded on 2026-11-27 so the
backend work can start once the design lands.

## Goal

The chat Tools popup shows only the user's working connectors, plus those that
stopped working without the user disconnecting them, so the user knows exactly
when to reconnect.

## Requirements

The frontend loads the user's popup list in one call. The list contains only:

- connectors that are connected and working;
- connectors that stopped working without the user disconnecting them.

It never contains:

- connectors the user disconnected in Settings, even while that disconnect is
  still finishing;
- connectors the user never connected;
- connectors an admin removed or disabled.

"Stopped working" means the user must sign in again to use it. A temporary
problem (network error, provider briefly down) or a sign-in that renews itself
automatically does not count.

Each connector in the list gives:

- its display name and logo;
- whether it is working or needs reconnecting;
- whether the user has it switched on for chats.

The on/off choice is saved per user and is the same on every device and in every
new chat. New connections start switched on. After a successful reconnect the
connector shows as working again. When the model tries to use a connector that
needs reconnecting, the frontend gets a signal it can recognise, so it can show
the in-chat reconnect prompt.

## Done when

- A connector the user disconnected never appears in the list.
- A connector whose access the user removed at the provider (for example in
  their Google account) appears as needs-reconnect.
- A brief network or provider error does not make a connector appear as
  needs-reconnect.
- A connector whose sign-in renews automatically stays working.
- Connectors an admin removed or disabled never appear.
- The on/off choice persists across chats and devices.
- A reconnected connector shows as working.
- Using a needs-reconnect connector in chat gives a signal the frontend can
  recognise.

## Starting points in the code (to confirm when work starts)

- Needs-reconnect today is only discovered during a chat call: the stream sends
  `mcp_oauth_required` and the connector result summary reads
  `Sign-in required` (see `docs/connector-console-api.md`, Tool Results). The
  list needs a stored state instead, set only on a definite sign-in failure
  (revoked grant or failed refresh), never on a network error or provider outage.
- `connection_state` in `src/services/connector_console_helpers.rs` already
  returns `NeedsReconnect`, but only from token presence and expiry: a grant
  revoked at the provider still looks connected until a call fails. The broker
  reports that failure as `connector_reconnect_required` (409) in
  `src/services/connector_broker.rs`; persisting it is the missing piece. Check
  that a user disconnect still in progress is not mistaken for it.
- The per-user on/off choice needs its own persisted setting; nothing stores it
  today.
