use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, QueryOrder};
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::admin_embedding::EmbeddingConfigResponse,
    models::embedding_configs,
    state::SharedState,
};

const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODEL: &str = "text-embedding-3-small";
const DEFAULT_DIMENSIONS: i32 = 1536;

pub async fn get_or_create_embedding_config(
    app_state: &SharedState,
) -> Result<embedding_configs::Model, AuthError> {
    if let Some(model) = embedding_configs::Entity::find()
        .order_by_desc(embedding_configs::Column::UpdatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("embedding config query error: {e}");
            AuthError::DbTimeout
        })?
    {
        return Ok(model);
    }

    let is_enabled = app_state
        .check_ai_engine_is_enabled(DEFAULT_PROVIDER)
        .await
        .unwrap_or(false);

    let model = embedding_configs::ActiveModel {
        id: Set(Uuid::new_v4()),
        provider: Set(DEFAULT_PROVIDER.to_string()),
        model: Set(DEFAULT_MODEL.to_string()),
        dimensions: Set(Some(DEFAULT_DIMENSIONS)),
        is_enabled: Set(is_enabled),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    };

    model.insert(&app_state.database).await.map_err(|e| {
        eprintln!("embedding config insert error: {e}");
        AuthError::DbTimeout
    })
}

pub async fn model_to_response(
    app_state: &SharedState,
    model: &embedding_configs::Model,
) -> EmbeddingConfigResponse {
    let api_key_configured = app_state
        .settings
        .get_ai_engine_api_key(&model.provider)
        .await
        .is_some();
    let provider_enabled = app_state
        .check_ai_engine_is_enabled(&model.provider)
        .await
        .unwrap_or(false);
    EmbeddingConfigResponse {
        provider: model.provider.clone(),
        model: model.model.clone(),
        dimensions: model.dimensions,
        is_enabled: model.is_enabled,
        api_key_configured,
        provider_enabled,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}
