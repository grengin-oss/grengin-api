// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{
    Json,
    extract::{Query, State},
};
use reqwest::StatusCode;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use std::collections::HashSet;

use crate::{
    auth::claims::Claims,
    dto::models::{ModelInfo, ModelsQuery, ModelsResponse, ProviderInfo},
    models::{ai_engines, users},
    services::{
        chat_attachments::global_attachments,
        chat_stream_helpers::is_model_whitelisted,
        department_policies::{effective_allowed_models, model_policy_key},
        model_picker::{PickerMetadata, localize},
        models_cache::load_models_cache,
        provider_models::{
            add_provider_models, is_chat_selectable_model, sort_models_newest_first,
        },
    },
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/models",
    tag = "models",
    params(ModelsQuery),
    responses(
        (status = 200, body = ModelsResponse, description = "Providers and models, plus the attachment limits that apply to every model"),
        (status = 401),
    )
)]
pub async fn get_list_models(
    claims: Claims,
    State(app_state): State<SharedState>,
    Query(query): Query<ModelsQuery>,
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
            .map(|m| model_policy_key(&m.provider, &m.model))
            .collect::<HashSet<(String, String)>>()
    });

    let (status, providers, picker) = match load_models_cache(&app_state.req_client).await {
        Ok(cache) => (StatusCode::OK, cache.providers, cache.picker),
        Err(error) => {
            eprintln!("providers cache error: {error}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Vec::new(),
                PickerMetadata::built_in(),
            )
        }
    };
    let catalog_providers = providers.clone();

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
            .filter(is_chat_selectable_model)
            .filter(|model| is_model_whitelisted(&whitelist, &[&model.key, &model.name]))
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

    for descriptor in app_state.provider_registry.descriptors().await {
        let provider_key = descriptor.id.to_string();
        let Some(plugin) = app_state.provider_registry.get(&descriptor.id).await else {
            continue;
        };
        if plugin.models().is_none() {
            continue;
        }
        let whitelist = app_state
            .settings
            .get_ai_engine_whitelist(&provider_key)
            .await
            .unwrap_or_default();
        if whitelist.is_empty() {
            continue;
        }
        let plugin_models = match app_state
            .live_models_cache
            .get_or_fetch(&provider_key, plugin.as_ref())
            .await
        {
            Ok(models) => models,
            Err(error) => {
                eprintln!(
                    "provider plugin model listing failed: {}",
                    crate::services::provider_chat::provider_error_class(&error)
                );
                continue;
            }
        };
        let mut models = plugin_models
            .into_iter()
            .map(|model| crate::services::provider_models::to_model_info(&provider_key, model))
            .map(|model| picker.enrich_plugin_model(model, &catalog_providers))
            .filter(is_chat_selectable_model)
            .filter(|model| is_model_whitelisted(&whitelist, &[&model.key, &model.name]))
            .collect::<Vec<_>>();
        if let Some(allowed) = &allowed_set {
            models.retain(|model| {
                allowed.contains(&model_policy_key(&provider_key, &model.key))
                    || allowed.contains(&(provider_key.clone(), model.name.to_lowercase()))
            });
        }
        if models.is_empty() {
            continue;
        }
        add_provider_models(
            &mut filtered_providers,
            ProviderInfo {
                key: provider_key,
                name: descriptor.name,
                icon: String::new(),
                icon_dark: String::new(),
                status: "enabled".to_string(),
                models,
            },
        );
    }

    if filtered_providers
        .iter()
        .any(|provider| provider.key == "grengin")
    {
        let direct_keys = ai_engines::Entity::find()
            .select_only()
            .column(ai_engines::Column::EngineKey)
            .filter(ai_engines::Column::IsEnabled.eq(true))
            .filter(ai_engines::Column::ApiKey.is_not_null())
            .into_tuple::<String>()
            .all(&app_state.database)
            .await
            .unwrap_or_default();
        let mut owned = HashSet::new();
        for direct in filtered_providers
            .iter()
            .filter(|provider| direct_keys.contains(&provider.key))
        {
            owned.extend(
                direct
                    .models
                    .iter()
                    .map(|model| format!("{}/{}", direct.key, model.key)),
            );
        }
        for gateway in filtered_providers
            .iter_mut()
            .filter(|provider| provider.key == "grengin")
        {
            gateway.models.retain(|model| !owned.contains(&model.key));
        }
        filtered_providers.retain(|provider| !provider.models.is_empty());
    }
    sort_models_newest_first(&mut filtered_providers);
    localize(&mut filtered_providers, query.locale.as_deref());
    (
        status,
        Json(ModelsResponse {
            providers: filtered_providers,
            global_attachments: global_attachments(&app_state.settings.attachments),
            specialties: picker.specialties(),
            price_tiers: picker.price_tiers(),
        }),
    )
}
