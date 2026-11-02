# Connector OAuth: Latest Docs And Sync Audit

Date: 2026-11-02. Scope: the committed single-instance connector integration in
grengin-api and the OAuth broker in grengin-service. This review fetched all
three repositories' latest origin refs, consulted current primary documentation
and reran the local suites. No production behavior was changed.

## Verdict

The branches are not integrated with origin/main. The pre-push snapshot below
also records the local commits awaiting publication when the review ran.
The existing pinned-provider suite passes, but three additional strict-protocol
probes reproduce two interoperability/security-hardening gaps. Do not equate the
passing provider fixtures with full current MCP authorization conformance.

The earlier three runtime fixes remain valid. The additional probes below cover
contracts that the previous suite did not enforce. Shared staging rollout still
requires integration review; no merge, push, deployment or R2 change was made.

The user subsequently reported that the local Slack reconnect worked and
approved pushing the reviewed feature/catalog branches. That approval does not
resolve the protocol gaps or authorize merging to main or deployment.

## Findings

### High Deployment Risk: Unsynced Branches Have Merge Conflicts

Pre-push review snapshot; use current Git refs to verify publication:

| Repository | Local HEAD | Remote feature HEAD | Unpushed commits | Main commits missing |
| --- | --- | --- | --- | --- |
| grengin-api | 28aca2b | beb836c | 2 | 1, at b365aca |
| grengin-website/service | 0680fc3 | d116b8e | 2 | 55, at f357f45 |
| grengin-list release/0.9.7 | f3c0600 | Release branch not present on origin | 1 relative to main | 0, main at 6f72280 |

At review time both feature branches contained every commit on their
corresponding remote feature branch; their two latest commits were local only.
Neither contained all of main. This is not just a behind-count warning:
`git merge-tree --write-tree --name-only HEAD origin/main` reports conflicts.

API conflicts: Cargo.toml and Cargo.lock, caused by the release-version changes.
Service conflicts:

- provisioning/aws/handler.ts
- provisioning/aws/hatchery-execution.ts
- provisioning/aws/runtime.ts
- scripts/hatchery-runtime.test.mjs, modified locally but deleted on main
- workers/src/index.ts

The service branch carries older provisioning/frontend-deployment logic that
main has subsequently moved. Do not deploy the whole old checkout over newer
shared staging. Resolve main into the feature branches, preserving the connector
and introspection additions alongside current main behavior, then retest. No
actual merge or conflict resolution was performed by this review.

### Medium Compatibility Gap: Resource Not Bound Through OAuth Requests

Source: service connector-broker.ts, oauthRequest at line 229, start at 335,
and callback exchange at 487. Neither default authorization nor code/refresh
token exchanges set a canonical resource. Operator-approved extra_params can
put resource in the authorization URL, but do not add it to token requests.

Both strict-provider probes return callback HTTP 503 and a failed connection
because the mock authorization server rejects a missing token-request resource.
The authorization-only configuration does not work around this.

The current [MCP authorization specification](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization#resource-parameter-implementation)
requires resource indicators in authorization and token requests. See also
[RFC 8707](https://www.rfc-editor.org/rfc/rfc8707.html).

Proposed fix: bind the reviewed canonical resource to the pending grant and carry
it into authorization, code exchange and refresh. Discover/validate it under the
existing operator trust policy; do not accept arbitrary browser-supplied resource
URLs or weaken endpoint allowlists. Validate resource changes as a consent/binding
change. Add strict resource-required code and refresh tests before claiming this
compatibility. No production fix for this expanded scope was applied here.

### Medium Hardening Gap: Callback Issuer Is Not Validated

Source: service connector-broker.ts callback at line 470. The stored pending
payload binds PKCE, callback and client ID, but does not record an expected
issuer. Callback reads state/code/error and ignores iss. The metadata fields
needed to enforce the advertised issuer-response policy are not loaded.

The isolated probe supplies an unexpected iss: the broker still performs one
token exchange and returns HTTP 200 with awaiting_confirmation. It does not
automatically activate a connection; owner confirmation remains required. This
is a validation-gap reproduction, not a demonstrated account takeover.

The current [MCP response-validation contract](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization#authorization-response-validation)
requires recording the validated issuer and checking a returned iss before
transmitting the code. See [RFC 9207](https://www.rfc-editor.org/rfc/rfc9207.html).
Provider-specific callback paths already provide a separate mix-up defense
described in [RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html#section-4.4.2.2);
the finding does not assume that every existing provider is exploitable.

Proposed fix: add operator-approved metadata/issuer binding, validate returned
iss exactly and enforce its advertised presence policy before code exchange,
including error callbacks. Preserve explicit support for reviewed vendor OAuth
flows rather than silently treating all catalog entries as fully discovered MCP
authorization servers. Test wrong, missing-required and valid issuer responses.

## Current Documentation And Live Metadata Checks

- The MCP latest authorization URL resolves to the 2026-07-28 specification.
- [Slack's guide](https://docs.slack.dev/ai/slack-mcp-server/) still specifies
  Streamable HTTP and the user-only authorization/token endpoints used here.
  It does not offer legacy SSE or dynamic client registration. Check its
  application eligibility requirements before enabling a public integration.
- Live unauthenticated Slack metadata requests succeeded. Its
  [protected-resource document](https://mcp.slack.com/.well-known/oauth-protected-resource)
  identifies resource `https://mcp.slack.com`, not the transport's `/mcp` URL.
  Its [authorization-server document](https://mcp.slack.com/.well-known/oauth-authorization-server)
  identifies issuer `https://mcp.slack.com`, the configured user OAuth endpoints
  and S256. Do not guess the resource by blindly copying the transport URL.
- [Slack's user token method](https://docs.slack.dev/reference/methods/oauth.v2.user.access/)
  still documents the user-token shape supported by the existing adapter.
- [RFC 7009](https://www.rfc-editor.org/rfc/rfc7009.html) remains the reference for
  configured standard revocation endpoints. This review does not establish
  real vendor revocation or fresh user consent.

## Catalog Sync

Both canonical 0.9.7 CDN files return HTTP 200, match the recorded SHA-256 hashes
and return CORS `*`. The index still pins 89d5127 with 18 AI providers and seven
auth templates. Git confirms the current backend runtime files, manifests and
migrations are unchanged from that pin; the later API commit adds tests/docs.

At review time origin/main in grengin-list did not contain the 0.9.7 directory,
and the published files were recorded only on the local release/0.9.7 branch.
Pushing that release branch makes the source available for review, but does not
integrate it into main. Preserve
the exact published bytes when integrating that branch; do not regenerate or
rewrite the released index to point at a new merge commit.

## Tests And Reproduction

The fresh normal run passed 758 Rust tests, 414 Worker tests, 53 compiled-backend
HTTP checks, the workspace build, Worker typecheck, both migration suites and
Slack configuration preflight. It used at most two Cargo jobs, isolated local
storage and mock vendor/model endpoints. Evidence:
`/tmp/grengin-connector-core-audit.EF0ecf/`.

The standards probes use the actual Worker handler and in-memory D1 fixture,
not real vendor credentials. Their normal pinned-provider control succeeds;
three strict-contract checks fail, representing the two gaps above. These are
separate from the passing normal suite and intentionally return exit status 1
until the missing protocol contracts are implemented:

```bash
bash tests/connectors/audit-core.sh
bash tests/connectors/protocol-audit.sh
```

Probe evidence: `/tmp/grengin-connectors-latest-protocol-audit-2026-11-02.log`.
No fresh browser consent, vendor token exchange, real model-led Slack tool call,
shared database migration or merged-result test was performed. Existing user
files and running local services were left unchanged. The report/probes are
recorded in a follow-up test/documentation commit; production code is unchanged.
