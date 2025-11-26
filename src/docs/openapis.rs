use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::auth::error::{ErrorResponse, ErrorDetail, ErrorDetailVariant};
use crate::docs::security::ApiSecurityAddon;
use crate::dto::auth::{LoginResponse};
use crate::dto::oauth::OidcProvider;
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
            Claims,
            OidcProvider
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