// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! Enabling and disabling the Grengin engine. Local routing opens only after the service has
//! confirmed model access, and only if no newer intent or deletion superseded the request.
use chrono::Utc;
use llm_plugin::ProviderError;
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::Set,
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    sea_query::{Expr, OnConflict},
};
use uuid::Uuid;

use crate::{
    auth::error::AuthError,
    dto::admin_ai::{
        GrenginCatalog, GrenginCatalogModel, GrenginConnectionState, GrenginFundingSource,
        GrenginOperationStatus, GrenginStatus,
    },
    models::{
        ai_engines, grengin_engine_lifecycle as lifecycle,
        grengin_engine_operations::{self as operations, OperationState},
    },
    services::{
        grengin_grant::{GRENGIN_CONSENT_VERSION, GrenginGrant},
        grengin_provider::split_model,
        provider_models::model_type,
        provider_resolver::own_key_covers,
        provider_runtime::{
            catalog_config, compile_engine, register_provider, unregister_provider,
        },
    },
    state::SharedState,
};

const ENGINE_KEY: &str = "grengin";
const LIFECYCLE_ROW: i16 = 1;

pub fn require_consent(enable: bool, consent_version: Option<&str>) -> Result<(), AuthError> {
    if enable && consent_version != Some(GRENGIN_CONSENT_VERSION) {
        return Err(AuthError::InvalidRequest {
            field: "consent_version",
        });
    }
    Ok(())
}

// Serializes every intent across backend processes: the row lock is the coordination point.
async fn lock_lifecycle(txn: &DatabaseTransaction) -> Result<lifecycle::Model, AuthError> {
    lifecycle::Entity::insert(lifecycle::ActiveModel {
        id: Set(LIFECYCLE_ROW),
        generation: Set(0),
        revision: Set(0),
        updated_at: Set(Utc::now()),
    })
    .on_conflict(
        OnConflict::column(lifecycle::Column::Id)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(txn)
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    lifecycle::Entity::find_by_id(LIFECYCLE_ROW)
        .lock_exclusive()
        .one(txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .ok_or(AuthError::DbTimeout)
}

async fn set_local_enabled<C: ConnectionTrait>(db: &C, enabled: bool) -> Result<u64, AuthError> {
    ai_engines::Entity::update_many()
        .col_expr(ai_engines::Column::IsEnabled, Expr::value(enabled))
        .col_expr(ai_engines::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
        .filter(ai_engines::Column::PluginConfig.is_null())
        .exec(db)
        .await
        .map(|result| result.rows_affected)
        .map_err(|_| AuthError::DbTimeout)
}

/// Records the intent, closes routing at once for a disable, asks the service, and opens
/// routing for an enable only once confirmed. Returns the operation while it is not applied.
pub async fn set_enabled(
    state: &SharedState,
    enabled: bool,
) -> Result<(Option<GrenginOperationStatus>, bool), AuthError> {
    let db = &state.database;
    let operation = record(db, enabled).await?;
    if !enabled {
        refresh_local_engine(state).await?;
    }

    let outcome = remote_set(enabled).await;
    let finished = complete(db, operation, enabled, outcome).await?;
    let local_enabled = refresh_local_engine(state).await?;
    Ok((
        (finished.state != OperationState::Applied).then(|| operation_status(&finished)),
        local_enabled,
    ))
}

async fn record(db: &DatabaseConnection, enabled: bool) -> Result<operations::Model, AuthError> {
    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let current = lock_lifecycle(&txn).await?;
    let revision = current.revision + 1;
    let mut life: lifecycle::ActiveModel = current.clone().into();
    life.revision = Set(revision);
    life.updated_at = Set(Utc::now());
    life.update(&txn).await.map_err(|_| AuthError::DbTimeout)?;
    if !enabled {
        set_local_enabled(&txn, false).await?;
    }
    let now = Utc::now();
    let operation = operations::ActiveModel {
        id: Set(Uuid::new_v4()),
        generation: Set(current.generation),
        revision: Set(revision),
        desired_enabled: Set(enabled),
        state: Set(OperationState::Applying),
        error_code: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await
    .map_err(|_| AuthError::DbTimeout)?;
    txn.commit().await.map_err(|_| AuthError::DbTimeout)?;
    Ok(operation)
}

async fn remote_set(enabled: bool) -> Result<(), (OperationState, String)> {
    let grant = GrenginGrant::configured()
        .map_err(|_| (OperationState::Failed, "setup_needed".to_owned()))?;
    grant.set_models_enabled(enabled).await.map_err(classify)?;
    if enabled {
        // Enabled service states include on, low, empty and pending: an empty balance is not
        // a failed enable. Only a status that cannot be read leaves confirmation pending.
        let engine_state = grant
            .status()
            .await
            .map_err(|_| (OperationState::Pending, "status_unavailable".to_owned()))?;
        if engine_state == "off" {
            return Err((OperationState::Pending, "remote_still_off".to_owned()));
        }
    }
    Ok(())
}

fn classify(error: ProviderError) -> (OperationState, String) {
    match error {
        ProviderError::HttpStatus { status, message } if status < 500 && status != 429 => {
            (OperationState::Failed, message)
        }
        ProviderError::Configuration(_) => (OperationState::Failed, "connection_not_ready".into()),
        _ => (OperationState::Pending, "service_unavailable".into()),
    }
}

async fn complete(
    db: &DatabaseConnection,
    operation: operations::Model,
    enabled: bool,
    outcome: Result<(), (OperationState, String)>,
) -> Result<operations::Model, AuthError> {
    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let life = lock_lifecycle(&txn).await?;
    let current = life.generation == operation.generation && life.revision == operation.revision;
    let (state, error_code) = match outcome {
        _ if !current => (OperationState::Failed, Some("superseded".to_owned())),
        Ok(()) if enabled => {
            if set_local_enabled(&txn, true).await? == 1 {
                (OperationState::Applied, None)
            } else {
                (OperationState::Failed, Some("not_installed".to_owned()))
            }
        }
        Ok(()) => (OperationState::Applied, None),
        Err((state, code)) => (state, Some(code)),
    };
    let mut active: operations::ActiveModel = operation.into();
    active.state = Set(state);
    active.error_code = Set(error_code);
    active.updated_at = Set(Utc::now());
    let finished = active
        .update(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    txn.commit().await.map_err(|_| AuthError::DbTimeout)?;
    Ok(finished)
}

/// Deletes the local engine and starts a new generation: no outstanding or late operation
/// can reopen routing or recreate the row. The service keeps its consent and balance.
pub async fn delete_engine(state: &SharedState, engine_id: Uuid) -> Result<(), AuthError> {
    close_installation(&state.database, engine_id).await
}

async fn close_installation(db: &DatabaseConnection, engine_id: Uuid) -> Result<(), AuthError> {
    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let current = lock_lifecycle(&txn).await?;
    let mut life: lifecycle::ActiveModel = current.clone().into();
    life.generation = Set(current.generation + 1);
    life.revision = Set(current.revision + 1);
    life.updated_at = Set(Utc::now());
    life.update(&txn).await.map_err(|_| AuthError::DbTimeout)?;
    operations::Entity::update_many()
        .col_expr(
            operations::Column::State,
            Expr::value(OperationState::Failed),
        )
        .col_expr(operations::Column::ErrorCode, Expr::value("superseded"))
        .col_expr(operations::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(
            operations::Column::State.is_in([OperationState::Applying, OperationState::Pending]),
        )
        .exec(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    ai_engines::Entity::delete_by_id(engine_id)
        .exec(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    txn.commit().await.map_err(|_| AuthError::DbTimeout)
}

async fn refresh_local_engine(state: &SharedState) -> Result<bool, AuthError> {
    let engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    state.live_models_cache.invalidate(ENGINE_KEY).await;
    let Some(engine) = engine else {
        unregister_provider(state, ENGINE_KEY).await;
        state.settings.remove_ai_engine_from_state(ENGINE_KEY).await;
        return Ok(false);
    };
    state
        .settings
        .load_ai_engine_in_state(
            ENGINE_KEY,
            None,
            engine.is_enabled,
            engine.whitelist_models.clone(),
        )
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    if engine.is_enabled {
        register_provider(state, &engine)
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    } else {
        unregister_provider(state, ENGINE_KEY).await;
    }
    Ok(engine.is_enabled)
}

fn operation_status(operation: &operations::Model) -> GrenginOperationStatus {
    GrenginOperationStatus {
        id: operation.id,
        revision: operation.revision,
        desired_enabled: operation.desired_enabled,
        state: operation.state,
        error_code: operation.error_code.clone(),
    }
}

async fn latest_unapplied(
    db: &DatabaseConnection,
) -> Result<Option<GrenginOperationStatus>, AuthError> {
    let latest = operations::Entity::find()
        .order_by_desc(operations::Column::Revision)
        .one(db)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    Ok(latest
        .filter(|operation| operation.state != OperationState::Applied)
        .map(|operation| operation_status(&operation)))
}

/// The service catalog with local policy and own-key precedence applied. Fresh on every call.
pub async fn catalog(state: &SharedState) -> Result<GrenginCatalog, AuthError> {
    let engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
        .filter(ai_engines::Column::PluginConfig.is_null())
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let package = state
        .discovery_catalog
        .ai_provider(ENGINE_KEY, None)
        .await
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let provider = compile_engine(
        catalog_config(package.plugin),
        ENGINE_KEY,
        None,
        Some(&state.discovery_catalog),
    )
    .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let listed = provider
        .models()
        .ok_or(AuthError::ServiceTemporarilyUnavailable)?
        .list_models()
        .await
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let observed_at = Utc::now();
    let mut models = Vec::with_capacity(listed.len());
    for model in listed {
        let id = model.id.as_str().to_owned();
        let Ok((provider_id, native_model_id)) = split_model(&id) else {
            continue;
        };
        let own_key = own_key_covers(state, provider_id, native_model_id)
            .await
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
        let price = |field: &str| model.metadata.get(field).and_then(|value| value.as_i64());
        models.push(GrenginCatalogModel {
            provider_id: provider_id.to_owned(),
            native_model_id: native_model_id.to_owned(),
            name: model.name.clone(),
            capability: model_type(&model),
            input_per_million_microdollars: price("inputPerMillionMicrodollars"),
            output_per_million_microdollars: price("outputPerMillionMicrodollars"),
            processing_region: None,
            status: "available".into(),
            is_allowed: engine
                .as_ref()
                .is_some_and(|engine| engine.whitelist_models.contains(&id)),
            is_default: engine.as_ref().is_some_and(|engine| {
                engine.default_model == id
                    || engine.default_image_gen_model.as_deref() == Some(id.as_str())
            }),
            effective_engine: if own_key {
                provider_id.to_owned()
            } else {
                ENGINE_KEY.into()
            },
            funding_source: if own_key {
                GrenginFundingSource::OwnKey
            } else {
                GrenginFundingSource::Grengin
            },
            id,
        });
    }
    Ok(GrenginCatalog {
        observed_at,
        models,
    })
}

/// Safe local and service status, available even when the identity or the local row is missing.
pub async fn status(state: &SharedState) -> Result<GrenginStatus, AuthError> {
    let engine = ai_engines::Entity::find()
        .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
        .filter(ai_engines::Column::PluginConfig.is_null())
        .one(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let mut status = GrenginStatus {
        installed: engine.is_some(),
        local_enabled: engine.as_ref().is_some_and(|engine| engine.is_enabled),
        connection_state: GrenginConnectionState::SetupNeeded,
        engine_state: None,
        remote_enabled: None,
        installation_id: None,
        workspace_id: None,
        balance_microdollars: None,
        available_microdollars: None,
        low_balance_microdollars: None,
        minimum_version: None,
        balance_url: None,
        top_up_url: None,
        observed_at: None,
        stale: false,
        error_code: None,
        pending_operation: latest_unapplied(&state.database).await?,
    };
    let grant = match GrenginGrant::configured() {
        Ok(grant) => grant,
        Err(_) => {
            // "Connection setup needed", never an upgrade requirement.
            status.engine_state = Some("update_needed".into());
            status.error_code = Some("setup_needed".into());
            return Ok(status);
        }
    };
    match grant.workspace().await {
        Ok(workspace) => {
            status.connection_state = GrenginConnectionState::Connected;
            status.engine_state = Some(workspace.engine_state.into());
            status.remote_enabled = Some(workspace.engine_state != "off");
            status.installation_id = workspace.installation_id;
            status.workspace_id = workspace.workspace_id;
            status.balance_microdollars = workspace.balance_microdollars;
            status.available_microdollars = workspace.available_microdollars;
            status.low_balance_microdollars = workspace.low_balance_microdollars;
            status.balance_url = workspace.balance_url;
            status.top_up_url = workspace.top_up_url;
            status.observed_at = Some(Utc::now());
        }
        Err(error) => {
            let disconnected = matches!(
                &error,
                ProviderError::HttpStatus {
                    status: 401 | 403,
                    ..
                }
            ) || matches!(&error, ProviderError::Configuration(message) if message.contains("revoked") || message.contains("different service installation"));
            status.connection_state = if disconnected {
                GrenginConnectionState::Disconnected
            } else {
                GrenginConnectionState::Unavailable
            };
            status.engine_state = disconnected.then(|| "disconnected".into());
            status.stale = true;
            status.error_code = Some(
                if disconnected {
                    "disconnected"
                } else {
                    "service_unavailable"
                }
                .into(),
            );
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ai_engines::ApiKeyStatus;

    #[test]
    fn enabling_needs_the_current_consent_version_and_disabling_needs_none() {
        assert!(require_consent(true, Some("grengin-models-v1")).is_ok());
        assert!(require_consent(true, None).is_err());
        assert!(require_consent(true, Some("grengin-models-cloud-v1")).is_err());
        assert!(require_consent(false, None).is_ok());
    }

    #[test]
    fn only_definite_service_answers_fail_an_operation() {
        let rejected = ProviderError::HttpStatus {
            status: 409,
            message: "workspace_disconnected".into(),
        };
        assert_eq!(
            classify(rejected),
            (OperationState::Failed, "workspace_disconnected".into())
        );
        for unsure in [
            ProviderError::HttpStatus {
                status: 503,
                message: "x".into(),
            },
            ProviderError::HttpStatus {
                status: 429,
                message: "x".into(),
            },
            ProviderError::Transport("timeout".into()),
        ] {
            assert_eq!(classify(unsure).0, OperationState::Pending);
        }
    }

    async fn database() -> DatabaseConnection {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        let db = sea_orm::Database::connect(&url)
            .await
            .expect("test database");
        ai_engines::Entity::delete_many()
            .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
            .exec(&db)
            .await
            .unwrap();
        db
    }

    async fn install(db: &DatabaseConnection) -> Uuid {
        let now = Utc::now();
        ai_engines::ActiveModel {
            id: Set(Uuid::new_v4()),
            display_name: Set("Grengin".into()),
            is_enabled: Set(false),
            engine_key: Set(ENGINE_KEY.into()),
            api_key_status: Set(ApiKeyStatus::NotConfigured),
            api_key: Set(None),
            api_key_validated_at: Set(None),
            whitelist_models: Set(vec!["openai/gpt-5".into()]),
            default_model: Set("openai/gpt-5".into()),
            default_image_gen_model: Set(None),
            plugin_config: Set(None),
            base_url_override: Set(None),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap()
        .id
    }

    async fn enabled(db: &DatabaseConnection) -> Option<bool> {
        ai_engines::Entity::find()
            .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
            .one(db)
            .await
            .unwrap()
            .map(|engine| engine.is_enabled)
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn routing_opens_only_for_the_latest_confirmed_intent_of_the_current_installation() {
        let db = database().await;
        let engine = install(&db).await;

        // Confirmed by the service: routing opens.
        let operation = record(&db, true).await.unwrap();
        let done = complete(&db, operation, true, Ok(())).await.unwrap();
        assert_eq!(done.state, OperationState::Applied);
        assert_eq!(enabled(&db).await, Some(true));

        // A disable closes routing in the same transaction that records it.
        let disable = record(&db, false).await.unwrap();
        assert_eq!(enabled(&db).await, Some(false));
        let failed = complete(
            &db,
            disable,
            false,
            Err((OperationState::Pending, "service_unavailable".into())),
        )
        .await
        .unwrap();
        assert_eq!(failed.state, OperationState::Pending);
        assert_eq!(enabled(&db).await, Some(false));

        // An older enable that completes after a newer intent never reopens routing.
        let older = record(&db, true).await.unwrap();
        let newer = record(&db, false).await.unwrap();
        let late = complete(&db, older, true, Ok(())).await.unwrap();
        assert_eq!(late.state, OperationState::Failed);
        assert_eq!(late.error_code.as_deref(), Some("superseded"));
        assert_eq!(enabled(&db).await, Some(false));
        complete(&db, newer, false, Ok(())).await.unwrap();

        // Deleting starts a new generation: a pending enable can neither reopen nor recreate it.
        let pending = record(&db, true).await.unwrap();
        close_installation(&db, engine).await.unwrap();
        assert_eq!(enabled(&db).await, None);
        let stored = operations::Entity::find_by_id(pending.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.error_code.as_deref(), Some("superseded"));
        let after_delete = complete(&db, pending, true, Ok(())).await.unwrap();
        assert_eq!(after_delete.state, OperationState::Failed);
        assert_eq!(enabled(&db).await, None);

        // A new install starts disabled and does not resume the old intent.
        install(&db).await;
        assert_eq!(enabled(&db).await, Some(false));
        assert!(
            latest_unapplied(&db)
                .await
                .unwrap()
                .is_some_and(|op| op.state == OperationState::Failed)
        );
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn the_reserved_engine_key_is_unique() {
        let db = database().await;
        install(&db).await;
        let second = ai_engines::ActiveModel {
            id: Set(Uuid::new_v4()),
            ..ai_engines::Entity::find()
                .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
                .one(&db)
                .await
                .unwrap()
                .unwrap()
                .into()
        };
        assert!(second.insert(&db).await.is_err());
    }
}
