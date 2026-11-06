# Native Connector Live Testing

Date: 2026-11-05. Local branch: feat/connector-server-plugin.
No commits, uploads, deployment, frontend changes or changes to the existing
0.9.7 API process/database were made for these preflights.

## Microsoft Graph Permissions

Choose Microsoft Graph DELEGATED permissions on a Web OAuth app, not application
permissions. Add only the plugins/tools you intend to enable.

| Plugin | Declared Graph permissions |
| --- | --- |
| Outlook Mail | Mail.Read |
| Outlook Calendar | Calendars.Read |
| OneDrive | Files.Read |
| SharePoint | Sites.Read.All |
| Microsoft 365 search (Graph-backed Work IQ entry) | User.Read, Files.Read.All, Mail.Read, Calendars.Read, Sites.Read.All |
| Teams | Team.ReadBasic.All, Channel.ReadBasic.All, Chat.ReadBasic, Chat.Read, ChannelMessage.Read.All, TeamMember.Read.All, ChannelMember.Read.All, Presence.Read.All |

Request offline_access for refresh tokens. Use tenant admin consent where required
by the permission or organization policy. No Application.ReadWrite.All,
Directory.ReadWrite.All or User.Read.All is needed for these read-only plugins.
Register http://localhost:5173/mcp/oauth/callback as a Web callback for local
connector testing, not the separate sign-in callback. Production needs its exact
HTTPS frontend callback. Teams, SharePoint and Graph search use work/school accounts.

References:
- https://learn.microsoft.com/en-us/graph/permissions-overview
- https://learn.microsoft.com/en-us/graph/permissions-reference
- https://learn.microsoft.com/en-us/graph/auth-v2-user
- https://learn.microsoft.com/en-us/graph/api/search-query?view=graph-rest-1.0

## Live Preflight Results

Run: bash tests/connector-plugin/live-preflight.sh
The script sources the approved local secret file without printing it and sends
deliberately invalid authorization codes. It never follows redirects, logs token
bodies/secrets or authorizes a real account. All responses are bounded.

| Provider | Observed result | Meaning |
| --- | --- | --- |
| Slack | HTTP 200 invalid_code | Expected invalid-code rejection; no token issued |
| Google | HTTP 400 invalid_grant | Expected invalid-code rejection; no token issued |
| GitHub | HTTP 200 bad_verification_code after callback correction | App uses its registered /v1/connectors/callback/github path |
| Jira | Initially identical client ID/secret, then HTTP 400 invalid_request with corrected local values | Dummy-code rejection alone does not establish successful consent |

These results do NOT prove successful consent, a valid secret, registered redirect
URIs for every provider, scopes, refresh or actual vendor tool calls. Those require
real browser authorization and subsequent authenticated API reads. Earlier Navikt
success is mock evidence only, not a replacement for these live tests.

## Next Steps

The process on port 8080 is the older isolated 0.9.7 broker test API. Do not report
it as the new 0.9.8 native-plugin runtime or restart it without coordinating local
testing. The current webapp on 5173 is /home/anurag/work/fresh/webapp.
Use a separate native backend/test DB and explicitly select that backend for the
frontend before consent. Otherwise its MCP callback reaches the wrong database.

The registered callbacks confirmed by the operator are:

```text
http://localhost:5173/v1/connectors/callback/jira
http://localhost:5173/v1/connectors/callback/github
http://localhost:5173/v1/connectors/callback/gmail
http://localhost:5173/v1/connectors/callback/google-calendar
http://localhost:5173/v1/connectors/callback//google-drive
```

The Drive double slash is intentional here: it matches the operator's existing
Google app registration exactly. It must not be collapsed during authorization
or code exchange. Slack alone continues to use /mcp/oauth/callback.

The native test Vite configuration redirects exactly those five paths to the existing
/mcp/oauth/callback page, preserving the query. The token exchange still uses the
original registered redirect_url in the MCP configuration, not the handoff page.
The alias is local-only under GRENGIN_LOCAL_NATIVE_PLUGINS=1 and has no-store and
no-referrer headers. Non-GET callbacks are rejected. It is not a deployed Worker
broker endpoint and does not bypass the signed-in backend callback.

## Isolated Setup Now Running

- Real webapp: http://localhost:5173/settings?tab=integrations
- Native backend: http://127.0.0.1:8083/ reports 0.9.8 and matching DB migration head.
- Six explicit native installs: Jira, GitHub, Slack, Gmail, Calendar and Drive; 39 tools.
- Fresh grengin_native_plugins_live DB in a separate 384 MiB PostgreSQL container.
- Separate APP_KEY/JWT_SECRET. Only the existing test admin's profile/identity was
  copied; no chats, files, OAuth tokens, grants or original password/MFA secrets.
- Existing API on 8080 still reports 0.9.7; original database/Worker stay unchanged.
- Metadata on 18084 serves a local test copy of the draft with fixture source pins;
  the actual catalog files/pins are not changed or published.
- Native API/helper unit capped at 1 GiB; native webapp capped at 768 MiB. No Docker
  image build, additional frontend dependencies or frontend commit.
- State/backups/logs are private under ~/.local/state/grengin/native-plugin-live.

Verification performed: frontend proxy targets 0.9.8; all six actual authorization
APIs emit the configured callback and list the expected read-only tools; probe
states are removed; alias headers/redirects and POST rejection pass; anonymous
connection requests are rejected; original 8080 API stays healthy. Vite TypeScript
configuration check, Node/bash syntax, Rust probe build and strict clippy pass.

## Manual Consent Progress

- GitHub: operator completed the real flow successfully. An early setup-helper
  configure call unnecessarily re-saved unchanged secrets and reset that new test
  session before live reads. A reconnect is needed; it is NOT recorded as a live
  GitHub tool-call pass. No original account session was modified.
- The helper now decrypts/compares the existing test app secret and omits unchanged
  client fields. Re-running configure preserved Slack's connected state.
- Jira: the initially duplicated client ID/secret was corrected in the local source
  file, applied only to the isolated server and its disabled test entry re-enabled.
  Actual Jira consent is pending. cloud_id must come from accessible resources.
- Slack: real native consent succeeded. get_team, list_conversations and list_users
  passed against Slack using the actual Rust plugin runtime and encrypted user token.
- Gmail, Google Calendar and Google Drive: user consent still pending at this snapshot.
  Their callback configurations were updated to the operator's exact registered
  per-provider paths without re-saving app secrets. Slack's connected session was
  preserved. Authorization URL, local alias, privacy-header, POST-rejection and
  exact URI parsing checks pass for all three. Real Google consent remains pending.

## Completed Live Tool Checks

This section supersedes the earlier consent-progress snapshot above: all six native
connectors are now connected after the operator completed real browser consent.

Sixteen real read-only tool calls through the Rust plugin runtime passed:

| Connector | Successful real API reads |
| --- | --- |
| Jira | get_me, list_projects |
| GitHub | get_me, list_repositories, get_repository, list_branches, list_commits |
| Slack | get_team, list_conversations, list_users |
| Gmail | list_messages, list_labels |
| Google Calendar | list_calendars, list_events |
| Google Drive | list_files, list_recent_files |

Jira's cloud_id was selected from its single actually authorized accessible resource;
no tools used the initial zero-UUID configuration. API results/content and tokens
were withheld from terminal output.

The actual backend POST /chat/stream path then passed six additional tests:

| Connector | Tool called in chat |
| --- | --- |
| Jira | get_me |
| GitHub | get_me |
| Slack | get_team |
| Gmail | list_labels |
| Google Calendar | list_calendars |
| Google Drive | list_files |

Every test verified the actual tool_call/tool_result SSE pair, success status,
the model receiving the result and resuming, final done, and exactly one execution
row for the new conversation with the expected server, tool, user ID/email,
timestamp, nonnegative duration and is_error=false. Vendor HTTP requests and
per-user OAuth tokens were real. Only the tool-selecting LLM was a temporary
loopback fixture; no paid LLM, real-model selection-quality or production deployment
claim is made. The temporary AI engine was removed after the run; its local HTTP
fixture was stopped. Test chats/execution records remain in the isolated test DB.

The first chat-harness run counted automatic title generation as a third tool-loop
request, and tried to parse JSON from a successful HTTP 204 engine deletion. Those
were test-harness errors, not failed vendor calls. The harness now handles title
generation separately and HTTP 204 correctly; the clean rerun passed 6/6 and exited 0.

These are live smoke tests, not live coverage of all 39 tools or every available
resource/permission. Resource-specific and write/delete tests are not implied.

The Rust live_probe example reads its credentials only from stdin and prints just
success/error and result shape/count, never vendor content or tokens. This validates
real provider reads, not a paid LLM chat completion. probe returns failure if any
actual read fails and does not count unconnected providers as passes.

## Reproduce And Restore

### Microsoft Native Consent Preparation

The same isolated 0.9.8 API now explicitly installs six Microsoft Graph plugins
(42 read-only tools): Outlook Mail (5), Outlook Calendar (4), OneDrive (4),
SharePoint (10), Teams (14), and Graph-backed Microsoft 365 Search (5). This is
direct native OAuth, not a staging Worker broker deployment. Existing six vendor
connections are retained.

Credentials are read from CONNECTOR_MICROSOFT_CLIENT_ID/SECRET if configured,
otherwise MICROSOFT_CLIENT_ID/SECRET, otherwise the approved AZURE_CLIENT_ID/SECRET.
No credential values are written to the report. Each connector uses its exact
registered http://localhost:5173/v1/connectors/callback/<connector-id> URI. The local
Vite aliases route only to the existing signed-in MCP callback page, preserving
the original redirect URI for token exchange. No frontend application commit is
made for these development-only aliases.

Preparation checks passed: 11 callback unit tests, all six real authorize APIs,
all six callback handoffs/method/privacy-header checks, authenticated route and
proxy checks, and all six enabled Connect buttons on the real page with real API
responses. Live invalid-code probes return invalid_grant (Microsoft diagnostic
9002313), with no tokens. This does not establish successful consent, scope grants,
secret validity, refresh, Microsoft 365 entitlements or actual Graph tool calls.
Real browser consent is still required before those can be reported as passing.

```bash
ms=(outlook-mail outlook-calendar onedrive sharepoint microsoft-teams microsoft-work-iq)
bash tests/connector-plugin/live-native.sh install "${ms[@]}"
bash tests/connector-plugin/live-native.sh check "${ms[@]}"
bash tests/connector-plugin/live-native.sh ui "${ms[@]}"
bash tests/connector-plugin/live-preflight.sh "${ms[@]}"
# Complete the six real browser consent flows in Settings / Integrations first:
bash tests/connector-plugin/live-native.sh probe "${ms[@]}"
bash tests/connector-plugin/live-native.sh chat "${ms[@]}"
```

install is idempotent and does not overwrite existing server configuration or
tokens. configure is the separate explicit operation to apply changed app settings.
probe reports pending or expired credentials as unsuccessful, never as live passes.
chat uses actual user tokens/vendor HTTP calls and only a local model fixture.

### Microsoft Live Result And Staging Scope

The operator deferred Microsoft consent/admin approval and asked to move forward
with the existing verified native connectors for staging. Do not block staging on
those Microsoft accounts or label them as successfully live-tested.

- OneDrive: real OAuth consent completed and an encrypted user connection exists.
  Both get_drive and list_root_files returned HTTP 400; get_drive also failed in
  the actual chat tool loop. A direct fixed GET /v1.0/me/drive outside the plugin
  returned the same HTTP 400 / BadRequest with a SharePoint Online licence-related
  diagnostic. This is not a successful OneDrive tool test; account type/authority,
  tenant provisioning and licensing need investigation before enabling it for staging.
- Outlook Mail, Outlook Calendar, SharePoint, Teams and Microsoft 365 Search:
  real consent/tool calls remain pending; some need tenant-admin approval.
- Staging's live-verified set is Jira, GitHub, Slack, Gmail, Google Calendar and
  Google Drive. Existing connections are preserved. Microsoft test installs remain
  in the isolated local DB for later testing; none is automatically installed on staging.
- Local frontend aliases are development-only. No staging frontend or service
  deployment, source push or R2 publication occurred during Microsoft preparation.

Run only the currently approved live set while Microsoft consent is deferred:

```bash
verified=(jira github slack gmail google-calendar google-drive)
bash tests/connector-plugin/live-native.sh chat "${verified[@]}"
bash tests/connector-plugin/live-native.sh probe "${verified[@]}"
```

The source branches were pushed before this follow-up: backend aff0cf5 and catalog
46ea9f3, both feat/connector-server-plugin. Backend version 0.9.8 is prepared, not
released: its distribution is still a draft with null source pins. Staging needs
the matching reviewed/published compatibility catalog before relying on discovery.
R2 publication requires its separate explicit approval; these tests do not publish it.

```bash
bash tests/connector-plugin/live-native.sh status
bash tests/connector-plugin/live-native.sh check
bash tests/connector-plugin/live-native.sh probe
bash tests/connector-plugin/live-native.sh chat
# After changing the approved local app credentials:
bash tests/connector-plugin/live-native.sh configure
# Restrict a callback/credential refresh to selected test connectors:
bash tests/connector-plugin/live-native.sh configure gmail google-calendar google-drive
```

The probe resolves Jira's cloud_id automatically only when exactly one authorized
Jira resource exists. Multiple sites require an explicit selection, not a guess.
It does not run Jira tools with the initial zero-UUID configuration.

Restore the previous local webapp after testing:

```bash
systemctl --user stop grengin-native-plugin-webapp
bash tests/connector-plugin/live-native.sh restore-webapp
systemctl --user stop grengin-native-plugin-live
docker stop grengin-native-plugin-live-db
```

The isolated DB container and private test state are retained so consent data is
not silently destroyed. Original webapp environment/arguments are restored from
the private backup; do not publish that backup or its session.json.
