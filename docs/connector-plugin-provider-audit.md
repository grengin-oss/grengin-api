# Connector Provider Contract Audit

Historical eight-provider audit. See the [all-34 report](connector-plugins-all-34-report.md)
for the expanded packages, resolved OAuth findings and current test evidence.

Reviewed: 2026-11-04. Branch: `feat/connector-server-plugin`.
Scope: the eight current REST plugins, with **read-only parity first**, as approved.
Local changes only. No commits, catalog changes, R2 uploads or deployments.

## Findings

1. **Coverage was incomplete, not equivalent to the hosted connector list.** The
   initial 16 tools were a starter subset. Added 38 documented read operations;
   all 54 have request fixtures, scopes and direct links to official REST docs.
   There are still deferred/partial scenarios below. This is not full parity.
2. **The catalog is not a REST specification.** Its Microsoft entries point to
   Agent 365 MCP servers/scopes, not Graph. Current Microsoft reference pages
   redirect to Copilot Studio and describe those app-specific servers as
   backward-compatibility previews with changing names. Do not copy MCP endpoint
   names or scopes into Graph requests. The new runtime remains Graph v1.0.
3. **Some read operations use POST.** Calendar free/busy is a read-only POST, not
   a mutation. Classification uses the manifest annotation, not the verb.
4. **Calendar scopes could be narrower.** Replaced the broad Google Calendar
   scope declarations with calendar-list, event-read and free/busy scopes per
   tool. Existing broad read grants remain valid vendor-side; new integrations
   should request the reviewed scope union in coverage.json.
5. **Response semantics differ from hosted MCP.** Gmail metadata is not decoded
   message content; availability is not an automatic meeting-time suggestion;
   file metadata is not document content. These remain explicitly partial or
   deferred rather than being advertised as working hosted tools.

All original 16 REST method/path choices were found in current official docs.
The audit expanded coverage and tightened declarations; it did not find evidence
that those endpoints had been removed. Outlook message reads now request a text
body using the documented Prefer header. Mail search avoids incompatible
search/orderby combinations; calendar event ordering retains singleEvents=true.

## Categories

- **Read Only**: `read_only: true`. All 54 provider tools in this batch.
- **Read & Write**: `read_only: false`. None enabled in this provider batch.
- `ConnectorManifest::tool_groups()` derives groups; `GET /mcp/tools` already
  exposes `is_read_only`. No API field or migration is needed to group them in UI.
- Existing MCP authorization enforces read-only versus full access. A category
  does not grant access or OAuth scopes. Destructive flags remain separate.
- The authoring example's draft-creation tool is Read & Write, and the test checks
  it is not misclassified. The free/busy POST is checked as Read Only.
- All **95 catalog write/delete entries** are deferred and non-executable in
  these manifests. They were not replaced by no-op tools or silently enabled.

## Coverage Against Saved Catalog

Catalog source: local grengin-list commit
`19fc8a59aa6a43a74e004f2f6d34a212778c3d03`.
Only the eight matching connector files were read; that repository was not edited.

| Plugin | Executable read tools | Comparable catalog read scenarios | Partial | Deferred |
| --- | ---: | ---: | ---: | ---: |
| outlook-mail | 5 | 2 | 2 | 1 |
| outlook-calendar | 4 | 1 | 1 | 6 |
| onedrive | 4 | 3 | 0 | 6 |
| sharepoint | 10 | 9 | 0 | 8 |
| microsoft-teams | 14 | 14 | 0 | 4 |
| gmail | 7 | 2 | 4 | 0 |
| google-calendar | 5 | 4 | 1 | 0 |
| google-drive | 5 | 3 | 1 | 2 |
| **Total** | **54** | **38** | **9** | **27** |

A comparable scenario means the same basic read operation, **not** a hosted MCP
name/input/output alias. Extra REST tools can exist without a catalog counterpart.
Graph collections currently fetch one page and retain nextLink; Google collections
have explicit page-token inputs. These are not exhaustive collection exports.

`grengin-list/master-data/connector-plugins/coverage.json` contains every
legacy catalog read/write name and an explicit mapping/status, plus the read tools
listed in the current official MCP documentation. Configure guides can be
non-exhaustive, and static docs are not a live `tools/list` response. Differences
flag review work; they do not prove a vendor removed a tool. No live MCP schema was
fetched with user credentials in this audit.

## Main Remaining Read Gaps

- Gmail full MIME/body decoding and hosted thread hydration/view links.
- Drive/OneDrive/SharePoint text and binary document downloads, export and redirect
  handling. Current metadata tools cannot honestly substitute for these.
- Graph function/colon addressing for drive search and site-path lookup; shared-link
  resolution and asynchronous operation handles need separately reviewed support.
- Teams natural-language/KQL search wrappers, multi-call file listing and static
  message-format reference tools.
- Outlook meeting-time suggestions, availability, room/mailbox settings and licensed
  meeting insights/transcripts/attendance APIs.
- Google automatic meeting suggestions: free/busy is a building block only.

## Official Sources

Microsoft MCP reference status and tool lists:
[Mail](https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-mail-tools),
[Calendar](https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-calendar-tools),
[OneDrive](https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-onedrive-tools),
[SharePoint](https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-sharepoint-tools),
[Teams](https://learn.microsoft.com/en-us/microsoft-copilot-studio/mcp-teams-tools).

Google hosted MCP prerequisites/tool-list references:
[Gmail](https://developers.google.com/workspace/gmail/api/guides/configure-mcp-server),
[Calendar](https://developers.google.com/workspace/calendar/api/guides/configure-mcp-server),
[Drive](https://developers.google.com/workspace/drive/api/guides/configure-mcp-server).
These hosted MCP services are different from the REST plugins; the plugins do not
require enabling a hosted MCP service or use its endpoint.

### Per-Tool REST Contracts

Each source was checked for method, path, delegated permission and supported
query/body shape. Tool limits can be lower than vendor maxima to bound resources.
The app's safety limits still apply to response size and concurrency.

### outlook-mail

- `list_messages`: `GET /me/messages`; `Mail.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/user-list-messages?view=graph-rest-1.0).
- `get_message`: `GET /me/messages/{message_id}`; `Mail.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/message-get?view=graph-rest-1.0).
- `search_messages`: `GET /me/messages`; `Mail.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/search-query-parameter).
- `list_sent_messages`: `GET /me/mailFolders/sentitems/messages`; `Mail.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/user-list-messages?view=graph-rest-1.0).
- `list_attachments`: `GET /me/messages/{message_id}/attachments`; `Mail.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/message-list-attachments?view=graph-rest-1.0).

### outlook-calendar

- `list_calendars`: `GET /me/calendars`; `Calendars.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/user-list-calendars?view=graph-rest-1.0).
- `list_events`: `GET /me/calendar/calendarView`; `Calendars.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/calendar-list-calendarview?view=graph-rest-1.0).
- `get_event`: `GET /me/events/{event_id}`; `Calendars.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/event-get?view=graph-rest-1.0).
- `list_calendar_events`: `GET /me/calendars/{calendar_id}/events`; `Calendars.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/user-list-events?view=graph-rest-1.0).

### onedrive

- `list_root_files`: `GET /me/drive/root/children`; `Files.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/driveitem-list-children?view=graph-rest-1.0).
- `get_file_metadata`: `GET /me/drive/items/{item_id}`; `Files.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/driveitem-get?view=graph-rest-1.0).
- `get_drive`: `GET /me/drive`; `Files.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/drive-get?view=graph-rest-1.0).
- `list_folder_files`: `GET /me/drive/items/{folder_id}/children`; `Files.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/driveitem-list-children?view=graph-rest-1.0).

### sharepoint

- `get_root_site`: `GET /sites/root`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/site-get?view=graph-rest-1.0).
- `list_site_lists`: `GET /sites/{site_id}/lists`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/list-list?view=graph-rest-1.0).
- `search_sites`: `GET /sites`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/site-search?view=graph-rest-1.0).
- `list_document_libraries`: `GET /sites/{site_id}/drives`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/drive-list?view=graph-rest-1.0).
- `get_default_library`: `GET /sites/{site_id}/drive`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/drive-get?view=graph-rest-1.0).
- `list_columns`: `GET /sites/{site_id}/lists/{list_id}/columns`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/list-list-columns?view=graph-rest-1.0).
- `list_items`: `GET /sites/{site_id}/lists/{list_id}/items`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/listitem-list?view=graph-rest-1.0).
- `get_item`: `GET /sites/{site_id}/lists/{list_id}/items/{item_id}`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/listitem-get?view=graph-rest-1.0).
- `list_folder_files`: `GET /drives/{drive_id}/items/{folder_id}/children`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/driveitem-list-children?view=graph-rest-1.0).
- `get_file_metadata`: `GET /drives/{drive_id}/items/{item_id}`; `Sites.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/driveitem-get?view=graph-rest-1.0).

### microsoft-teams

- `list_joined_teams`: `GET /me/joinedTeams`; `Team.ReadBasic.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/user-list-joinedteams?view=graph-rest-1.0).
- `list_channels`: `GET /teams/{team_id}/channels`; `Channel.ReadBasic.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/channel-list?view=graph-rest-1.0).
- `get_team`: `GET /teams/{team_id}`; `Team.ReadBasic.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/team-get?view=graph-rest-1.0).
- `get_channel`: `GET /teams/{team_id}/channels/{channel_id}`; `Channel.ReadBasic.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/channel-get?view=graph-rest-1.0).
- `list_chats`: `GET /me/chats`; `Chat.ReadBasic`. [Official operation](https://learn.microsoft.com/en-us/graph/api/chat-list?view=graph-rest-1.0).
- `get_chat`: `GET /chats/{chat_id}`; `Chat.ReadBasic`. [Official operation](https://learn.microsoft.com/en-us/graph/api/chat-get?view=graph-rest-1.0).
- `list_chat_messages`: `GET /chats/{chat_id}/messages`; `Chat.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/chat-list-messages?view=graph-rest-1.0).
- `get_chat_message`: `GET /chats/{chat_id}/messages/{message_id}`; `Chat.Read`. [Official operation](https://learn.microsoft.com/en-us/graph/api/chatmessage-get?view=graph-rest-1.0).
- `list_channel_messages`: `GET /teams/{team_id}/channels/{channel_id}/messages`; `ChannelMessage.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/channel-list-messages?view=graph-rest-1.0).
- `list_channel_replies`: `GET /teams/{team_id}/channels/{channel_id}/messages/{message_id}/replies`; `ChannelMessage.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/chatmessage-list-replies?view=graph-rest-1.0).
- `list_team_members`: `GET /teams/{team_id}/members`; `TeamMember.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/team-list-members?view=graph-rest-1.0).
- `list_channel_members`: `GET /teams/{team_id}/channels/{channel_id}/members`; `ChannelMember.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/channel-list-members?view=graph-rest-1.0).
- `list_chat_members`: `GET /chats/{chat_id}/members`; `Chat.ReadBasic`. [Official operation](https://learn.microsoft.com/en-us/graph/api/chat-list-members?view=graph-rest-1.0).
- `get_user_presence`: `GET /users/{user_id}/presence`; `Presence.Read.All`. [Official operation](https://learn.microsoft.com/en-us/graph/api/presence-get?view=graph-rest-1.0).

### gmail

- `list_messages`: `GET /users/me/messages`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list).
- `get_message_metadata`: `GET /users/me/messages/{message_id}`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/get).
- `search_threads`: `GET /users/me/threads`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/list).
- `get_thread_metadata`: `GET /users/me/threads/{thread_id}`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/get).
- `list_drafts`: `GET /users/me/drafts`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/list).
- `get_draft_metadata`: `GET /users/me/drafts/{draft_id}`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.drafts/get).
- `list_labels`: `GET /users/me/labels`; `https://www.googleapis.com/auth/gmail.readonly`. [Official operation](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.labels/list).

### google-calendar

- `list_calendars`: `GET /users/me/calendarList`; `https://www.googleapis.com/auth/calendar.calendarlist.readonly`. [Official operation](https://developers.google.com/workspace/calendar/api/v3/reference/calendarList/list).
- `list_events`: `GET /calendars/{calendar_id}/events`; `https://www.googleapis.com/auth/calendar.events.readonly`. [Official operation](https://developers.google.com/workspace/calendar/api/v3/reference/events/list).
- `get_event`: `GET /calendars/{calendar_id}/events/{event_id}`; `https://www.googleapis.com/auth/calendar.events.readonly`. [Official operation](https://developers.google.com/workspace/calendar/api/v3/reference/events/get).
- `search_events`: `GET /calendars/{calendar_id}/events`; `https://www.googleapis.com/auth/calendar.events.readonly`. [Official operation](https://developers.google.com/workspace/calendar/api/v3/reference/events/list).
- `query_free_busy`: `POST /freeBusy`; `https://www.googleapis.com/auth/calendar.events.freebusy`. [Official operation](https://developers.google.com/workspace/calendar/api/v3/reference/freebusy/query).

### google-drive

- `list_files`: `GET /files`; `https://www.googleapis.com/auth/drive.metadata.readonly`. [Official operation](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list).
- `get_file_metadata`: `GET /files/{file_id}`; `https://www.googleapis.com/auth/drive.metadata.readonly`. [Official operation](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/get).
- `search_files`: `GET /files`; `https://www.googleapis.com/auth/drive.metadata.readonly`. [Official operation](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list).
- `list_recent_files`: `GET /files`; `https://www.googleapis.com/auth/drive.metadata.readonly`. [Official operation](https://developers.google.com/workspace/drive/api/reference/rest/v3/files/list).
- `get_file_permissions`: `GET /files/{file_id}/permissions`; `https://www.googleapis.com/auth/drive.metadata.readonly`. [Official operation](https://developers.google.com/workspace/drive/api/reference/rest/v3/permissions/list).

## Verification

- Workspace: **794 passed, 0 failed, 61 ignored** in the default run.
- Provider/runtime/schema crate: **26 passed**, included above.
- Plugin HTTP: **32 passed**; existing broker HTTP: **54 passed**.
- Explicit isolated connector migration up/down/up: passed, in addition to the
  default run's tests. Development build, formatting, strict new-crate clippy
  and diff whitespace checks passed. The existing proc-macro-error2 future
  compatibility warning remains.

The repeatable command is:

```bash
bash tests/connector-plugin/run.sh --http --broker
```

Provider contract tests exercise all 54 tools for two users: **108 local HTTP
requests**, checking exact method/path/query/header/body and independent bearer
headers. Every tool also rejects missing auth and unexpected argument properties.
The inventory test checks categories, methods, paths, scopes and official source
hosts against the actual manifests; it rejects undocumented/misclassified mappings.

These are local protocol fixtures, not real vendor consent or successful paid/live
provider calls. Production account/tenant policy, licensing, restricted-scope
verification, quota and larger-than-limit responses still need staging validation.
The existing broker HTTP and migration regressions run against a disposable DB;
normal credentials and application data are not used or modified.
