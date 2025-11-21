use axum::{Router};
use utoipa::{OpenApi};
use utoipa_swagger_ui::SwaggerUi;
use crate::{docs::openapis::ApiDoc, state::SharedState};

pub fn swagger_ui_routes() -> Router<SharedState> {
    Router::new()
        .merge(SwaggerUi::new("/swagger-ui").url("/openapi.json", ApiDoc::openapi()))
}