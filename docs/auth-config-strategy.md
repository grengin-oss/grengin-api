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

The implementation supports any standards-compliant OIDC provider, including Google,
Microsoft Entra ID, Keycloak, Authentik, Okta, Dex, and compatible self-hosted systems. It also
supports GitHub OAuth Apps through a native, typed OAuth 2.0 social adapter.

- `sso_providers` is the source of truth; credentials remain encrypted with `APP_KEY`.
- `AppState` loads enabled providers into a runtime registry keyed by a validated provider slug.
- Google and Entra keep their compatibility adapters. Other providers use OIDC discovery.
- GitHub uses fixed GitHub authorization, token, user, and verified-email endpoints. Provider
  JSON may configure its presentation and least-privilege scopes, but may not replace those
  endpoints.
- Auth0 compatibility covers discovery, PKCE, standard profile claims, `offline_access`, and the
  configurable `audience`/organization authorization parameters used for API access and token
  reuse preparation.
- Keycloak compatibility covers realm discovery behavior, standard profile claims, group and
  `realm_access.roles` token shapes, custom scopes, and `kc_idp_hint` authorization parameters.
  Required/admin role enforcement and role synchronization remain part of the claim-mapping
  roadmap below; a successful Keycloak login must not be presented as role-mapping parity.
- Entra tenant-independent authorities (`common`, `organizations`, and `consumers`) validate the
  token tenant ID against both the token issuer and the selected signing key's issuer metadata.
  The verified tenant is security context only; persisted identity subjects retain their legacy
  representation so existing accounts continue to resolve.
- Entra tenant-independent JWKS metadata is cached by authority for 24 hours, with a short failure
  backoff and immediate invalidation when signature verification indicates key rotation.
- PKCE S256 remains mandatory except for the typed Apple web profile, whose published metadata
  does not advertise PKCE. State, nonce, ID token signature verification, issuer validation, and
  exact callback redirects remain mandatory for every OIDC profile.
- Returning users resolve by provider slug plus OIDC subject. The legacy Google and Entra ID
  columns are retained as a migration fallback.
- Email account linking is allowed only for verified provider email claims and can be disabled
  per provider.
- Custom providers are created disabled, validated, then explicitly enabled.

Public provider discovery uses `GET /auth/providers`. It returns every provider configured on the
installation with its name, slug, login path, enabled state, and auto-redirect preference. It
never returns client IDs, secrets, issuers, domains, or internal policy. Clients must only present
an enabled provider as an available login method.

Provider setup discovery is a separate, credential-free catalog exposed through
`GET /discovery/auth-providers` and `GET /discovery/auth-providers/{provider}`. The API filters the
public metadata catalog to contracts supported by this backend, resolves major or exact versions,
and verifies immutable templates before returning them. It is never used to determine which login
methods are configured or enabled on an installation.

## Provider Configuration

The `configuration` JSON document is versioned independently from the database schema:

```json
{
  "version": "1.1",
  "scopes": ["openid", "email", "profile", "groups"],
  "authorizationParams": {
    "prompt": "select_account"
  },
  "pkce": "s256",
  "emailLinking": "verifiedEmail",
  "autoRedirect": false
}
```

Rules:

- `openid` is mandatory for OIDC providers. The GitHub OAuth profile instead requires exactly
  `read:user` and `user:email`.
- Configuration `1.0` remains accepted and implies `pkce: "s256"`. Configuration `1.1` makes the
  mode explicit. Only the `apple` profile may select `pkce: "disabled"`; GitHub and every generic
  OIDC provider still require `s256`.
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
4. `GET /auth/providers` makes all configured providers and their enabled state discoverable to
   clients.
5. `GET /auth/{provider}` starts login and `/auth/{provider}/callback` completes it. Apple posts
   its URL-encoded callback to the API, which completes the exchange and redirects to the
   configured frontend callback with Grengin tokens in the URL fragment. The frontend removes
   the fragment before making a network request.

Managed Grengin proxy setup follows the same invariant through a separate probe because its
sentinel credentials cannot be validated against Google or Entra directly. The client first calls
`POST /admin/sso-providers/{id}/quick-setup/validate` with the intended domains and tenant, then
passes the returned token to `POST /admin/sso-providers/{id}/quick-setup`. That token is bound to
the admin, provider ID, normalized domains, tenant, and exact callback URL.

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

Adapters:

1. OIDC: current foundation; add configurable claim and role/group mapping.
2. OAuth 2.0 social profiles: GitHub is implemented with authorization code plus PKCE and
   verified identity data from `/user` and `/user/emails`. Additional non-OIDC vendors require
   their own typed profile rather than endpoint JSON.
3. LDAP/Active Directory: bind/search settings, StartTLS or LDAPS, username/email mapping, and no
   plaintext bind passwords outside encrypted storage.
4. SAML 2.0: signed assertions, audience/recipient/time validation, metadata rotation, and
   optional single logout.

The adapter enum is closed in a release, while provider records are open at runtime. This permits
arbitrary OIDC vendors today without pretending that OIDC JSON can safely implement LDAP or SAML.

## Social Provider Compatibility

Provider tests must follow the vendor's published metadata and authorization contract. A generic
OIDC mock must not be used to claim support for a provider whose real protocol differs.

- Okta is supported through the generic OIDC adapter. Use an org or custom authorization-server
  issuer, authorization code with S256 PKCE, and the `openid`, `email`, and `profile` scopes.
- LinkedIn is supported through the generic OIDC adapter. Use the live discovery issuer
  `https://www.linkedin.com/oauth` and the `openid`, `profile`, and `email` scopes. The email
  claim is optional; when LinkedIn omits it, Grengin uses a provider-scoped synthetic address and
  must not link the identity to an existing account by an unverified fallback.
- Sign in with Apple is supported for web clients through the typed `apple` profile. It uses
  issuer `https://appleid.apple.com`, scopes `openid`, `email`, and `name`,
  `response_mode=form_post`, and `pkce=disabled`. The registered HTTPS redirect URI points to the
  Grengin API callback, which validates the single-use state and nonce, exchanges and verifies the
  token, then redirects to the configured frontend callback. The client secret is Apple's
  developer-signed JWT; administrators must generate and rotate it before expiration. Native
  Apple SDK behavior remains outside this server-side OIDC profile.

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
- Microsoft multitenant tokens additionally validate the `tid` claim against the token and signing
  key issuers without changing the persisted provider subject.
- Automatic email linking requires a verified claim. Disabled linking requires an explicit admin
  or authenticated-user linking flow.
- Callback state is atomically consumed with `DELETE ... RETURNING` and expires after 15 minutes.
  Nonce is mandatory. PKCE S256 is mandatory except for the versioned Apple profile.
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
profiles at once. The focused OIDC smoke cases in this slice are:

- `/auth0`
- `/okta`
- `/keycloak`
- `/linkedin`
- `/apple`

Google OIDC and Microsoft Entra ID / Azure AD are already covered elsewhere and are intentionally
skipped here. Each issuer gets its own discovery document, token endpoint, and JWKS. Auth0, Okta,
Keycloak, and LinkedIn use this standard OIDC path in production. The Apple mock must exercise
`form_post`, current Apple scopes, an authorization request without PKCE, URL-encoded callback
parsing, and the API-to-frontend fragment handoff. It cannot prove Apple's developer-signed
client-secret JWT or Apple Developer HTTPS registration. GitHub is covered separately by its
native OAuth2 adapter tests. Vendor-native SDKs,
Graph/Admin APIs, native mobile login behavior, and other provider-specific edges require separate
integration tests.

OIDC protocol tests use a pinned `navikt/mock-oauth2-server` container to exercise discovery,
authorization-code issuance, PKCE, nonce propagation, signed ID tokens, code replay, exact
web/Android/iOS callback preservation, and Auth0/Keycloak-shaped authorization and claim profiles
without vendor credentials. Microsoft-specific multitenant issuer and signing-key issuer rules
remain deterministic Rust tests because they are Entra extensions rather than standard OIDC
metadata.

Run the credential-free OIDC protocol suite with `./tests/auth/mock_oauth2.sh`. The script starts
and removes the pinned container automatically; regular `cargo test` keeps these cases ignored.
