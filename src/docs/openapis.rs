use utoipa::OpenApi;
use crate::auth::claims::Claims;
use crate::docs::security::ApiSecurityAddon;
use crate::dto::auth::{LoginResponse,LoginType};
use crate::handlers::google_oauth;

#[derive(OpenApi)]
#[openapi(
    paths(
        google_oauth::google_login_start,
        google_oauth::google_oauth_callback,
    ),
    components(
        schemas(
            LoginResponse,
            LoginType,
            Claims,
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