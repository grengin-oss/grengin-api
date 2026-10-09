// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, DatabaseBackend, DatabaseConnection, EntityTrait,
    FromQueryResult, IntoActiveModel, QueryOrder, Statement,
};
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    config::setting::EmbeddingSettings,
    dto::admin_embedding::{EmbeddingConfigResponse, EmbeddingConfigUpdateRequest},
    models::embedding_configs,
    state::SharedState,
};

const DEFAULT_PROVIDER: &str = "openai";
const DEFAULT_MODEL: &str = "text-embedding-3-small";
const DEFAULT_DIMENSIONS: i32 = 1536;

#[derive(FromQueryResult)]
struct VectorColumnRow {
    dimensions: i32,
}

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

pub async fn apply_embedding_config_update(
    app_state: &SharedState,
    req: EmbeddingConfigUpdateRequest,
) -> Result<embedding_configs::Model, AuthError> {
    let config = get_or_create_embedding_config(app_state).await?;

    let provider_changed = req
        .provider
        .as_ref()
        .is_some_and(|provider| provider != &config.provider);
    let model_changed = req
        .model
        .as_ref()
        .is_some_and(|model| model != &config.model);
    if provider_changed || model_changed {
        return Err(AuthError::DbConflict);
    }
    if let Some(dimensions) = req.dimensions {
        let column_dimensions = load_vector_column_dimensions(&app_state.database).await?;
        if !dimensions_fit_vector_columns(dimensions, &column_dimensions) {
            return Err(AuthError::InvalidRequest {
                field: "dimensions",
            });
        }
    }

    let mut active = config.into_active_model();
    if let Some(dimensions) = req.dimensions {
        active.dimensions = Set(Some(dimensions));
    }
    if let Some(is_enabled) = req.is_enabled {
        active.is_enabled = Set(is_enabled);
    }
    active.updated_at = Set(Utc::now());

    let updated = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("embedding config update error: {e}");
        AuthError::DbTimeout
    })?;

    app_state
        .settings
        .set_embedding_config_in_state(EmbeddingSettings {
            provider: updated.provider.clone(),
            model: updated.model.clone(),
            dimensions: updated.dimensions,
            is_enabled: updated.is_enabled,
        })
        .await;

    Ok(updated)
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

async fn load_vector_column_dimensions(db: &DatabaseConnection) -> Result<Vec<i32>, AuthError> {
    // pg_attribute is a system catalog with no SeaORM entity; pgvector stores the declared dimension as the column typmod.
    let sql = r#"
        SELECT a."atttypmod" AS "dimensions"
        FROM pg_attribute a
        WHERE a."attrelid" IN (to_regclass('message_embeddings'), to_regclass('project_source_chunks'))
          AND a."attname" = 'embedding'
          AND NOT a."attisdropped"
    "#;
    let rows =
        VectorColumnRow::find_by_statement(Statement::from_string(DatabaseBackend::Postgres, sql))
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("vector column dimension query error: {e}");
                AuthError::DbTimeout
            })?;
    Ok(rows.into_iter().map(|row| row.dimensions).collect())
}

// A typmod of -1 means the column was declared as plain `vector` and accepts any dimension.
fn dimensions_fit_vector_columns(requested: i32, column_dimensions: &[i32]) -> bool {
    requested > 0
        && !column_dimensions.is_empty()
        && column_dimensions
            .iter()
            .all(|&declared| declared < 0 || declared == requested)
}

#[cfg(test)]
mod tests {
    use super::dimensions_fit_vector_columns;

    #[test]
    fn dimension_matching_every_vector_column_is_accepted() {
        assert!(dimensions_fit_vector_columns(1536, &[1536, 1536]));
    }

    #[test]
    fn dimension_differing_from_the_vector_columns_is_rejected() {
        assert!(!dimensions_fit_vector_columns(3072, &[1536, 1536]));
        assert!(!dimensions_fit_vector_columns(768, &[1536, 1536]));
    }

    #[test]
    fn dimension_is_rejected_when_any_vector_column_differs() {
        assert!(!dimensions_fit_vector_columns(1536, &[1536, 3072]));
    }

    #[test]
    fn non_positive_dimension_is_rejected() {
        assert!(!dimensions_fit_vector_columns(0, &[1536]));
        assert!(!dimensions_fit_vector_columns(-1, &[-1]));
    }

    #[test]
    fn unconstrained_vector_column_accepts_any_positive_dimension() {
        assert!(dimensions_fit_vector_columns(3072, &[-1, -1]));
        assert!(dimensions_fit_vector_columns(1536, &[-1, 1536]));
    }

    #[test]
    fn missing_vector_columns_reject_every_dimension() {
        assert!(!dimensions_fit_vector_columns(1536, &[]));
    }
}
