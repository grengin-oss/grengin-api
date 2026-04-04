# Schema Naming Cleanup

Guide for aligning backend schema names with the OpenAPI spec. The spec has already been updated to use the final names. The backend needs to match.

## Naming Convention

- **No `Response`/`Request`/`Dto`/`List` suffixes** — the path definition already indicates request vs response
- **`Create`/`Update` suffixes are kept** — they distinguish CRUD payloads from the base resource
- **`Status` is kept** when it describes a domain concept (e.g., `BudgetStatus`, `OnboardingStatus`)

## What stays as-is

**Create/Update variants:**
`AIModelCreate`, `AIModelUpdate`, `DepartmentCreate`, `DepartmentUpdate`, `RoleCreate`, `RoleUpdate`, `McpServerCreate`, `McpServerUpdate`, `UserCreate`, `UserUpdate`, `BudgetCreate`, `BrandingUpdate`, `EmbeddingConfigUpdate`, `SsoProviderCreate`, `SsoProviderUpdate`, `RateLimitConfigCreate`, `McpServerAccessUpdate`, `McpToolAccessUpdate`, `UserModelAccessUpdate`, `UserStatusUpdate`

**Status enums:**
`UserStatus`, `FileStatus`, `McpServerStatus`, `VirusScanStatus`

**Status objects:**
`RateLimitStatus`, `BudgetStatus`, `DepartmentBudgetStatus`, `OnboardingStatus`

## Full rename table

Each row shows the current backend struct name, the previous spec name, and the final name (now in the spec). Where the backend doesn't have an existing struct yet, the backend column shows `—`.

### Admin — AI Engines

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| `AiEngineResponse` | `AIEngineDetail` | `AIEngineDetail` |
| `AiEngineUpdateRequest` | `AIEngineUpdate` | `AIEngineUpdate` |
| `AiEngineValidationResponse` | `AIEngineValidateResponse` | `AIEngineValidation` |
| — | `AIEngineModelsResponse` | `AIEngineModels` |

### Admin — Departments

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| `DepartmentResponse` | `Department` | `Department` |
| `DepartmentRequest` | `DepartmentCreate` | `DepartmentCreate` |
| `DepartmentUpdateRequest` | `DepartmentUpdate` | `DepartmentUpdate` |
| `DepartmentTreeResponse` | `DepartmentTree` | `DepartmentTree` |
| `DepartmentBudgetStatusDto` | `DepartmentBudgetStatus` | `DepartmentBudgetStatus` |
| `DepartmentsListResponse` | inline `{ departments, total }` | inline `{ departments, total }` |
| `DepartmentMembersResponse` | inline `{ members, total }` | inline `{ members, total }` |
| `MoveDepartmentRequest` | `DepartmentMove` | `DepartmentMove` |
| — | `DepartmentMembersRequest` | `DepartmentMembersInput` |

### Admin — Users & RBAC

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| `UserResponse` | `PaginatedUsers` | `PaginatedUsers` |
| `UserRequest` | `UserCreate` | `UserCreate` |
| `UserUpdateRequest` | `UserUpdate` | `UserUpdate` |
| `UserDetails` | `User` | `User` |
| — | `UserRoleAssignmentRequest` | `UserRoleAssignmentInput` |

### Admin — SSO

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| `SsoProviderResponse` | `OidcProviderConfig` | `SsoProvider` |
| `SsoProviderEditableResponse` | `OidcProviderConfig` | `SsoProvider` (merge into one) |
| `SsoProviderUpdateRequest` | `OidcProviderConfigUpdate` | `SsoProviderUpdate` |

> **Note:** Renamed from `OidcProviderConfig` to `SsoProvider` to keep the schema protocol-agnostic. The provider object can carry a `type` field (e.g., `oidc`, `saml`, `ldap`) rather than baking the protocol into the schema name. The OIDC-specific fields (discovery_url, client_id, etc.) remain as properties — they're just not required for all provider types.

### Admin — Branding & Embedding

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| `BrandingResponse` | `Branding` | `Branding` |

### Admin — Analytics

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| — | `AnalyticsOverviewResponse` | `AnalyticsOverview` |
| — | `AnalyticsTimeSeriesResponse` | `AnalyticsTimeSeries` |
| — | `DepartmentAnalyticsResponse` | `DepartmentAnalytics` |
| — | `UserAnalyticsResponse` | `UserAnalytics` |

### Auth

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| — | `AuthInitResponse` | `AuthInit` |
| — | `AuthCallbackRequest` | `AuthCallback` |
| — | `AuthTokenResponse` | `AuthToken` |
| — | `RefreshTokenRequest` | `RefreshToken` |
| `AuthErrorResponse` | `Error` | `Error` |

### Onboarding

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| — | `OnboardingStartRequest` | `OnboardingStart` |
| — | `OnboardingStartResponse` | `OnboardingStartResult` |
| — | `OrganizationSetupRequest` | `OrganizationSetup` |
| — | `SuperAdminCreateRequest` | `SuperAdminCreate` |
| — | `LlmProviderSetupRequest` | `LlmProviderSetup` |
| — | `ValidateApiKeyRequest` | `ValidateApiKey` |
| — | `ValidateApiKeyResponse` | `ValidateApiKeyResult` |
| — | `OnboardingSsoRequest` | `OnboardingSso` |
| — | `OnboardingCompleteResponse` | `OnboardingComplete` |

### Password / MFA

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| — | `PasswordLoginRequest` | `PasswordLogin` |
| — | `PasswordLoginResponse` | `PasswordLoginResult` |
| — | `MfaSetupResponse` | `MfaSetup` |
| — | `MfaVerifyRequest` | `MfaVerify` |
| — | `MfaRecoveryRequest` | `MfaRecovery` |
| — | `PasswordForgotRequest` | `PasswordForgot` |
| — | `PasswordResetRequest` | `PasswordReset` |
| — | `PasswordChangeRequest` | `PasswordChange` |

### Common / Chat

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| — | `HealthResponse` | `Health` |
| — | `ChatRequest` | `ChatInput` |
| — | `ConversationList` | `PaginatedConversations` |

### MCP

| Backend (current) | Spec (previous) | Spec (now) |
|---|---|---|
| `McpServerAccessResponse` | `McpServerAccessList` | `McpServerAccess` |
| `BulkToolAccessUpdate` | inline object per spec | inline object per spec |
| `BulkToolAccessUpdateResponse` | inline object per spec | inline object per spec |
| — | `McpAccessDefaultRequest` | `McpAccessDefault` |
| — | `McpToolAccessList` | `McpToolAccess` |
| — | `McpToolsList` | `McpTools` |
| — | `McpUserConnectionsList` | `McpUserConnections` |
| — | `McpAuthorizeResponse` | `McpAuthorize` |
| — | `McpDisconnectResponse` | `McpDisconnect` |
| — | `McpOauthCallbackResponse` | `McpOauthCallback` |

## Endpoint cleanup

Remove `GET /admin/department/{department_id}` (singular). The spec uses only the plural form `GET /admin/departments/{department_id}`.

## Impact

- **Spec:** Done — all 37 renames applied
- **Backend:** ~23 struct renames or utoipa `#[schema(title)]` changes needed to match the **Spec (now)** column