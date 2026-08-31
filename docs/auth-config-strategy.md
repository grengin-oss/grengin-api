# Configurable Authentication Strategy

## Purpose

Grengin authentication must support self-hosted and enterprise identity systems without
hard-coding one login path per vendor. The target is feature parity with the useful parts of
LibreChat authentication while keeping protocol verification type-safe and auditable.

Provider configuration is data. Protocol implementations are Rust code. JSON may configure
issuer metadata, scopes, claim mappings, presentation, and policy, but it must never replace
OIDC token verification, PKCE, state/nonce validation, SAML signature verification, LDAP TLS,
redirect allowlisting, or secret encryption.

## Current Foundation

The first implementation slice supports any standards-compliant OIDC provider, including
Google, Microsoft Entra ID, Keycloak, Authentik, Okta, Dex, and compatible self-hosted systems.

- `sso_providers` is the source of truth; credentials remain encrypted with `APP_KEY`.
- `AppState` loads enabled providers into a runtime registry keyed by a validated provider slug.
- Google and Entra keep their compatibility adapters. Other providers use OIDC discovery.
- PKCE S256, state, nonce, ID token signature verification, issuer validation, and exact callback
  redirects remain mandatory.
- Returning users resolve by provider slug plus OIDC subject. The legacy Google and Entra ID
  columns are retained as a migration fallback.
- Email account linking is allowed only for verified provider email claims and can be disabled
  per provider.
- Custom providers are created disabled, validated, then explicitly enabled.

Public login discovery uses `GET /auth/providers`. It returns only the enabled provider name,
slug, login path, and auto-redirect preference. It never returns client IDs, secrets, issuers,
domains, or internal policy.

Provider setup discovery is a separate, credential-free catalog. Versioned templates live at
`https://meta.grengin.com/auth-providers/` and describe provider names, icons, documented issuer
patterns, required/recommended scopes, and safe configuration defaults. `GET
/auth/provider-templates` exposes the compact template index to clients with a five-minute cache
and stale-on-CDN-error fallback. It is never used to determine which login methods are enabled.

## Provider Configuration

The `configuration` JSON document is versioned independently from the database schema:

```json
{
  "version": "1.0",
  "scopes": ["openid", "email", "profile", "groups"],
  "authorizationParams": {
    "prompt": "select_account"
  },
  "emailLinking": "verifiedEmail",
  "autoRedirect": false
}
```

Rules:

- `openid` is mandatory and PKCE cannot be disabled.
- Reserved OAuth parameters such as `redirect_uri`, `client_id`, `state`, `nonce`, `scope`, and
  `code_challenge` cannot be overridden.
- HTTPS is mandatory except for loopback development URLs.
- Unknown JSON fields and unsupported configuration versions are rejected.
- Provider slugs use lowercase ASCII letters, digits, and hyphens, start with a letter, and are
  at most 63 characters.

## Administrative Flow

1. `POST /admin/sso-providers` creates a disabled provider with encrypted credentials.
2. `POST /admin/sso-providers/{id}/validate` validates URLs, configuration, discovery metadata,
   and vendor-specific credentials where a reliable probe exists. It returns a short-lived token
   bound to the admin, provider ID, and exact draft hash.
3. `PUT /admin/sso-providers/{id}` supplies that token for sensitive changes and enables the
   provider.
4. `GET /auth/providers` makes the enabled login methods discoverable to clients.
5. `GET /auth/provider-templates` lists the provider templates available to the admin UI.
6. `GET /auth/{provider}` starts login and `/auth/{provider}/callback` completes it.

An admin must not be able to enable a new or materially changed provider without validating the
same draft. Deleting a provider disables credentials and evicts it from runtime state; linked
identity history remains on users for audit and safe re-enablement.

## Target Architecture

Protocol support grows behind a typed internal interface:

```rust
#[async_trait]
trait AuthProtocolAdapter {
    fn protocol(&self) -> AuthProtocol;
    fn validate_config(&self, config: &AuthProviderRecord) -> Result<(), AuthConfigError>;
    async fn validate_remote(&self, config: &AuthProviderRecord) -> Result<(), AuthConfigError>;
    async fn begin(&self, request: LoginRequest) -> Result<LoginRedirect, AuthError>;
    async fn complete(&self, callback: LoginCallback) -> Result<VerifiedIdentity, AuthError>;
}
```

`VerifiedIdentity` is the only object account linking and JIT provisioning consume. It carries a
stable issuer/provider key, subject, verified-email state, display claims, groups, and optional
upstream tokens. Protocol adapters cannot write users directly.

Planned adapters:

1. OIDC: current foundation; add configurable claim and role/group mapping.
2. OAuth 2.0 social profiles: typed profiles for providers that do not expose OIDC identity
   tokens. PKCE remains required when supported.
3. LDAP/Active Directory: bind/search settings, StartTLS or LDAPS, username/email mapping, and no
   plaintext bind passwords outside encrypted storage.
4. SAML 2.0: signed assertions, audience/recipient/time validation, metadata rotation, and
   optional single logout.

The adapter enum is closed in a release, while provider records are open at runtime. This permits
arbitrary OIDC vendors today without pretending that OIDC JSON can safely implement LDAP or SAML.

## LibreChat Parity Roadmap

The next schema is a singleton `auth_settings` policy record, separate from provider credentials:

- Enable or disable local email/password login.
- Enable or disable local registration.
- Enable or disable social login and social registration independently.
- Registration domain allowlist.
- Access and refresh token lifetimes with bounded server-side limits.
- Login order and one-provider auto redirect.
- OIDC claim mappings, required/admin role mapping, group-to-role mapping, and role sync.
- Optional upstream access-token reuse with encrypted storage and explicit scopes.
- Provider logout and end-session behavior.
- LDAP and SAML adapters.

References:

- <https://www.librechat.ai/docs/configuration/authentication>
- <https://www.librechat.ai/docs/configuration/authentication/OAuth2-OIDC>
- <https://www.librechat.ai/docs/configuration/authentication/ldap>
- <https://www.librechat.ai/docs/configuration/authentication/SAML>

## Security and Privacy Invariants

- Secrets are write-only API inputs, encrypted at rest, redacted from logs, and returned only as
  previews.
- OIDC identities are keyed by provider and subject. Email is an attribute, never the primary
  external identity.
- Automatic email linking requires a verified claim. Disabled linking requires an explicit admin
  or authenticated-user linking flow.
- Callback state is single-use and expires after 15 minutes. PKCE and nonce are mandatory.
- Redirects are exact configured values; arbitrary request redirects are rejected.
- Provider configuration changes are permission checked and audit logged.
- The public provider catalog exposes documented issuer patterns only. It never exposes configured
  tenant IDs, installation domains, client IDs, credentials, or internal policy.
- JIT provisioning, domain restrictions, and account status checks apply uniformly to every
  adapter.
- Authentication logs contain provider slug and outcome, not authorization codes, tokens,
  secrets, or full claims.

## Test Contract

Every adapter and configuration version must cover:

- Valid and invalid provider slugs, URLs, versions, scopes, and reserved parameters.
- Missing, malformed, expired, replayed, wrong-issuer, wrong-audience, and wrong-nonce tokens.
- PKCE mismatch and callback redirect mismatch.
- Existing identity login, verified-email linking, disabled/unverified linking, JIT disabled, and
  deleted/suspended/pending users.
- Duplicate provider records and concurrent identity linking.
- Secret encryption/redaction and configuration hash invalidation.
- Provider disable, update, cache refresh, restart reload, and failed discovery behavior.
- Migration backfill and rollback for legacy Google and Entra identities.

Remote provider tests use local mock OIDC/LDAP/SAML servers in CI. Live vendor smoke tests are
optional staging checks and must never be the only coverage for protocol behavior.

The OIDC mock matrix is split by issuer path so one server can exercise several configured
profiles at once. The focused smoke cases in this slice are:

- `/auth0`
- `/okta`
- `/keycloak`
- `/apple`
- `/github`

Google OIDC and Microsoft Entra ID / Azure AD are already covered elsewhere and are intentionally
skipped here. Each issuer gets its own discovery document, token endpoint, and JWKS. Auth0, Okta,
and Keycloak use this standard OIDC path in production. The Apple and GitHub profiles only prove
that an OIDC-shaped issuer with their requested scopes and authorization parameters works; they do
not prove Sign in with Apple's client-secret JWT/form-post behavior or GitHub's OAuth2-only user
login flow. Vendor-native SDKs, Graph/Admin APIs, native mobile login behavior, and other
provider-specific edges require separate integration tests.
