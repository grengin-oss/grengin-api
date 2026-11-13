# All 34 Internal Connector Plugins

Date: 2026-11-05. Branch: feat/connector-server-plugin.
User target: every entry in grengin-list's 34-entry cloud catalog has a real
internal plugin implementation; remaining validation should be with vendor
credentials, not missing transport or authentication code. Read-only scope
remains as previously approved. No commit, merge, push or publication until audit.

## Work

- [x] Compare all 34 catalog IDs with the eight existing internal packages.
- [x] Review primary API references for the 26 missing providers.
- [x] Add bounded configuration, tenant URL and credential-delivery support.
- [x] Add encrypted per-user non-OAuth credential installation using existing storage.
- [x] Fix the eight reproduced OAuth review probes without distributed infrastructure.
- [x] Implement all 26 missing packages with meaningful read-only operations.
- [x] Add exact provider request fixtures and whole-catalog coverage checks.
- [x] Refresh versioned catalog metadata and the unpublishable 0.9.8 draft.
- [x] Run runtime, HTTP/chat, Navikt and catalog verification sequentially.

## Constraints

- Production JSON stays in grengin-list with immutable individual versions.
- Runtime features stay generic; no provider-name dispatch in backend Rust.
- Third-party OAuth uses the customer's application; plugins cannot borrow broker identity.
- API keys and tokens use existing encrypted per-user mcp_connections storage, never manifests.
- Shopify/Salesforce need tenant destinations, Shopify/Wix custom auth headers,
  Linear/Shopify fixed GraphQL read queries, and Mailchimp a private JSON-body key.
- Work IQ entry is explicitly a Graph-backed Microsoft 365 search implementation,
  not a clone of Microsoft's proprietary Work IQ engine or a licensing bypass.
- Mock success does not prove live consent, vendor permissions, licensing or account access.
- Rust builds: one job, 4 GiB no-swap cap when compilation is required. Mock
  containers are capped independently; do not modify other agents' work.
