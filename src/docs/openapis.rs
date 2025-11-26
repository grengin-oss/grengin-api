use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::auth::error::{ErrorResponse, ErrorDetail, ErrorDetailVariant};
use crate::docs::security::ApiSecurityAddon;
use crate::dto::auth::{AuthInitResponse, AuthTokenResponse, LoginResponse, TokenType, User, UserRole, UserStatus};
use crate::handlers::oidc;

#[derive(OpenApi)]
#[openapi(
    paths(
        oidc::oidc_login_start,
        oidc::oidc_oauth_callback,
    ),
    components(
        schemas(
            LoginResponse,
            AuthInitResponse,
            AuthTokenResponse,
            TokenType,
            User,
            UserRole,
            UserStatus,
            Claims,
            ErrorResponse,
            ErrorDetail,
            ErrorDetailVariant
        )
    ),
    tags(
        (name = "auth", description = "Authentication & user endpoints"),
        (name = "root", description = "Root / health"),
    ),
    modifiers(
        &ApiSecurityAddon
    )
)]
pub struct ApiDoc;