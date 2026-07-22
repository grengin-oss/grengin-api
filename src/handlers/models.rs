use axum::{Json, extract::State};
use reqwest::StatusCode;
use sea_orm::EntityTrait;
use std::collections::HashSet;

use crate::{
    auth::claims::Claims,
    dto::models::{ModelInfo, ModelType, ModelsResponse, ProviderInfo},
    models::users,
    services::{
        department_policies::effective_allowed_models,
        models_cache::load_providers_cached,
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/models",
    tag = "models",
    responses(
        (status = 200, body = ModelsResponse, description = "List of providers and models"),
        (status = 401),
    )
)]
pub async fn get_list_models(
    claims: Claims,
    State(app_state): State<SharedState>,
) -> (StatusCode, Json<ModelsResponse>) {
    let user = users::Entity::find_by_id(claims.user_id)
        .one(&app_state.database)
        .await
        .ok()
        .flatten();
    let allowed_models = if let Some(user) = &user {
        if let Some(dept_id) = user.department_id {
            effective_allowed_models(&app_state.database, dept_id)
                .await
                .ok()
                .flatten()
        } else {
            None
        }
    } else {
        None
    };
    let allowed_set = allowed_models.map(|models| {
        models
            .into_iter()
            .map(|m| (m.provider.to_lowercase(), m.model.to_lowercase()))
            .collect::<HashSet<(String, String)>>()
    });

    let (status, providers) = match load_providers_cached(&app_state.req_client).await {
        Ok(providers) => (StatusCode::OK, providers),
        Err(error) => {
            eprintln!("providers cache error: {error}");
            (StatusCode::SERVICE_UNAVAILABLE, Vec::new())
        }
    };

    let mut filtered_providers = Vec::new();
    for provider in providers {
        let is_enabled = app_state
            .check_ai_engine_is_enabled(&provider.key)
            .await
            .unwrap_or(false);
        if !is_enabled {
            continue;
        }
        let whitelist = app_state
            .settings
            .get_ai_engine_whitelist(&provider.key)
            .await
            .unwrap_or_default();
        if whitelist.is_empty() {
            continue;
        }
        let mut models = provider
            .models
            .into_iter()
            .filter(|model| model.model_type != ModelType::TextEmbedder)
            .filter(|model| whitelist.contains(&model.name) || whitelist.contains(&model.key))
            .collect::<Vec<ModelInfo>>();
        if let Some(allowed) = &allowed_set {
            let provider_key = provider.key.to_lowercase();
            models = models
                .into_iter()
                .filter(|model| {
                    allowed.contains(&(provider_key.clone(), model.key.to_lowercase()))
                        || allowed.contains(&(provider_key.clone(), model.name.to_lowercase()))
                })
                .collect();
        }
        if models.is_empty() {
            continue;
        }
        filtered_providers.push(ProviderInfo {
            key: provider.key,
            name: provider.name,
            icon: provider.icon,
            icon_dark: provider.icon_dark,
            status: provider.status,
            models,
        });
    }

    (
        status,
        Json(ModelsResponse {
            providers: filtered_providers,
        }),
    )
}
