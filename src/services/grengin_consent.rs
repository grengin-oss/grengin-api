// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! Managed creator installation and administrator consent changes for the Grengin engine.
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
        provider_resolver::own_key_covers_engine,
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

// Keep this in the creator transaction; prior installs and deletions must remain authoritative.
pub(super) async fn preinstall_for_managed_creator(
    txn: &DatabaseTransaction,
) -> Result<(), AuthError> {
    super::ai_plugin::lock_engine_installation(txn, ENGINE_KEY).await?;
    let life = lock_lifecycle(txn).await?;
    if life.generation == 0
        && life.revision == 0
        && ai_engines::Entity::find()
            .filter(ai_engines::Column::EngineKey.eq(ENGINE_KEY))
            .one(txn)
            .await
            .map_err(|_| AuthError::DbTimeout)?
            .is_none()
    {
        ai_engines::ActiveModel::native(ENGINE_KEY.into(), "Grengin".into(), true, Utc::now())
            .insert(txn)
            .await
            .map_err(|_| AuthError::DbTimeout)?;
    }
    Ok(())
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

/// The service calls this module makes. Abstracted so tests can use a slow or lossy service.
#[async_trait::async_trait]
trait ModelAccess: Send + Sync {
    async fn engine_state(&self) -> Result<String, ProviderError>;
    async fn set_enabled(&self, enabled: bool) -> Result<(), ProviderError>;
}

struct ServiceModelAccess(&'static GrenginGrant);

#[async_trait::async_trait]
impl ModelAccess for ServiceModelAccess {
    async fn engine_state(&self) -> Result<String, ProviderError> {
        self.0.status().await.map(str::to_owned)
    }

    async fn set_enabled(&self, enabled: bool) -> Result<(), ProviderError> {
        self.0.set_models_enabled(enabled).await
    }
}

fn service_access() -> Option<ServiceModelAccess> {
    GrenginGrant::configured().ok().map(ServiceModelAccess)
}

// After a superseded intent, the service is re-checked against the newest one for this long: a
// call whose reply was lost may still land after a newer intent was applied.
const VERIFY_AFTER_SUPERSEDED: chrono::Duration = chrono::Duration::minutes(10);
const RECONCILE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Records the intent, closes routing at once for a disable, then converges the service and
/// local routing to the newest intent. Returns this operation while it is not applied.
pub async fn set_enabled(
    state: &SharedState,
    enabled: bool,
) -> Result<(Option<GrenginOperationStatus>, bool), AuthError> {
    let db = &state.database;
    if !enabled {
        // Before waiting for the lifecycle lock, which a service call may hold for seconds.
        set_local_enabled(db, false).await?;
        refresh_local_engine(state).await?;
    }
    let operation = record(db, enabled).await?;
    let access = service_access();
    converge_with(
        db,
        access.as_ref().map(|access| access as &dyn ModelAccess),
        Utc::now(),
        LockWait::Wait,
    )
    .await?;
    let finished = operations::Entity::find_by_id(operation.id)
        .one(db)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .ok_or(AuthError::DbTimeout)?;
    let local_enabled = refresh_local_engine(state).await?;
    Ok((
        (finished.state != OperationState::Applied).then(|| operation_status(&finished)),
        local_enabled,
    ))
}

/// Takes the next revision. Older open intents are superseded and will never be sent again.
async fn record(db: &DatabaseConnection, enabled: bool) -> Result<operations::Model, AuthError> {
    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let current = lock_lifecycle(&txn).await?;
    let op = record_locked(&txn, current, enabled, OperationState::Applying).await?;
    txn.commit().await.map_err(|_| AuthError::DbTimeout)?;
    Ok(op)
}

async fn record_locked(
    txn: &DatabaseTransaction,
    current: lifecycle::Model,
    enabled: bool,
    state: OperationState,
) -> Result<operations::Model, AuthError> {
    let revision = current.revision + 1;
    let mut life: lifecycle::ActiveModel = current.clone().into();
    life.revision = Set(revision);
    life.updated_at = Set(Utc::now());
    life.update(txn).await.map_err(|_| AuthError::DbTimeout)?;
    supersede_open(txn).await?;
    if !enabled {
        set_local_enabled(txn, false).await?;
    }
    let now = Utc::now();
    operations::ActiveModel {
        id: Set(Uuid::new_v4()),
        generation: Set(current.generation),
        revision: Set(revision),
        desired_enabled: Set(enabled),
        state: Set(state),
        error_code: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(txn)
    .await
    .map_err(|_| AuthError::DbTimeout)
}

async fn supersede_open(txn: &DatabaseTransaction) -> Result<(), AuthError> {
    operations::Entity::update_many()
        .col_expr(
            operations::Column::State,
            Expr::value(OperationState::Failed),
        )
        .col_expr(operations::Column::ErrorCode, Expr::value("superseded"))
        .col_expr(operations::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(operations::Column::State.is_in([
            OperationState::Confirming,
            OperationState::Applying,
            OperationState::Pending,
        ]))
        .exec(txn)
        .await
        .map(|_| ())
        .map_err(|_| AuthError::DbTimeout)
}

#[derive(Clone, Copy, PartialEq)]
enum LockWait {
    Wait,
    // The background reconciler and status reads skip when another process is converging.
    Skip,
}

/// The only place that calls the service. It holds the lifecycle row lock for the whole
/// exchange, so calls are serialized across processes and always carry the newest intent: a
/// new intent cannot be recorded mid-call, and a superseded one is never sent again.
async fn converge_with(
    db: &DatabaseConnection,
    access: Option<&dyn ModelAccess>,
    now: chrono::DateTime<Utc>,
    wait: LockWait,
) -> Result<(), AuthError> {
    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let life = match wait {
        LockWait::Wait => lock_lifecycle(&txn).await?,
        LockWait::Skip => match try_lock_lifecycle(&txn).await? {
            Some(life) => life,
            None => return Ok(()),
        },
    };
    let latest = operations::Entity::find()
        .filter(operations::Column::Generation.eq(life.generation))
        .order_by_desc(operations::Column::Revision)
        .one(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let Some(latest) = latest else {
        return txn.commit().await.map_err(|_| AuthError::DbTimeout);
    };
    match latest.state {
        OperationState::Confirming => {
            let outcome = confirm_registration(access).await;
            finish(&txn, &life, latest, true, outcome).await?;
        }
        OperationState::Applying | OperationState::Pending => {
            let enabled = latest.desired_enabled;
            let outcome = match access {
                Some(access) => remote_converge(access, enabled).await,
                None => Err((OperationState::Failed, "setup_needed".to_owned())),
            };
            finish(&txn, &life, latest, enabled, outcome).await?;
        }
        OperationState::Applied if verification_due(&txn, &latest, now).await? => {
            if let Some(access) = access {
                // A disagreement left by a late call is corrected; a failure is retried next tick.
                let _ = remote_converge(access, latest.desired_enabled).await;
            }
        }
        OperationState::Applied | OperationState::Failed => {}
    }
    txn.commit().await.map_err(|_| AuthError::DbTimeout)
}

async fn confirm_registration(
    access: Option<&dyn ModelAccess>,
) -> Result<(), (OperationState, String)> {
    let access = access.ok_or((OperationState::Failed, "setup_needed".into()))?;
    let status = access.engine_state().await.map_err(|error| {
        let (state, code) = classify(error);
        // A retry must stay read-only; Pending dispatches the administrator consent path.
        (
            if state == OperationState::Pending {
                OperationState::Confirming
            } else {
                state
            },
            code,
        )
    })?;
    match status.as_str() {
        "on" | "low" | "empty" | "pending" => Ok(()),
        "off" => Err((OperationState::Failed, "remote_off".into())),
        _ => Err((OperationState::Confirming, "status_unavailable".into())),
    }
}

async fn try_lock_lifecycle(
    txn: &DatabaseTransaction,
) -> Result<Option<lifecycle::Model>, AuthError> {
    lifecycle::Entity::find_by_id(LIFECYCLE_ROW)
        .lock_with_behavior(
            sea_orm::sea_query::LockType::Update,
            sea_orm::sea_query::LockBehavior::SkipLocked,
        )
        .one(txn)
        .await
        .map_err(|_| AuthError::DbTimeout)
}

async fn verification_due(
    txn: &DatabaseTransaction,
    latest: &operations::Model,
    now: chrono::DateTime<Utc>,
) -> Result<bool, AuthError> {
    // Any generation: deleting and reinstalling the local engine keeps the same service
    // installation, so a call from before the deletion can still land. Revisions keep
    // increasing across generations, so "older" is well defined.
    operations::Entity::find()
        .filter(operations::Column::Revision.lt(latest.revision))
        .filter(operations::Column::ErrorCode.eq("superseded"))
        .filter(operations::Column::UpdatedAt.gte(now - VERIFY_AFTER_SUPERSEDED))
        .one(txn)
        .await
        .map(|found| found.is_some())
        .map_err(|_| AuthError::DbTimeout)
}

/// Reads the service first and sends the idempotent SET only when it disagrees.
async fn remote_converge(
    access: &dyn ModelAccess,
    enabled: bool,
) -> Result<(), (OperationState, String)> {
    let engine_state = access.engine_state().await.map_err(classify)?;
    if (engine_state != "off") == enabled {
        return Ok(());
    }
    access.set_enabled(enabled).await.map_err(classify)?;
    if enabled {
        // Enabled service states include on, low, empty and pending: an empty balance is not
        // a failed enable. Only a status that cannot be read leaves confirmation pending.
        let engine_state = access.engine_state().await.map_err(classify)?;
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

async fn finish(
    txn: &DatabaseTransaction,
    life: &lifecycle::Model,
    operation: operations::Model,
    enabled: bool,
    outcome: Result<(), (OperationState, String)>,
) -> Result<operations::Model, AuthError> {
    let current = life.generation == operation.generation && life.revision == operation.revision;
    let (state, error_code) = match outcome {
        _ if !current => (OperationState::Failed, Some("superseded".to_owned())),
        Ok(()) if enabled => {
            if set_local_enabled(txn, true).await? == 1 {
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
    active.update(txn).await.map_err(|_| AuthError::DbTimeout)
}

/// Finishes an operation outside convergence; the first decision stands and completion stays
/// generation/revision fenced.
#[cfg(test)]
async fn complete(
    db: &DatabaseConnection,
    operation: operations::Model,
    enabled: bool,
    outcome: Result<(), (OperationState, String)>,
) -> Result<operations::Model, AuthError> {
    let txn = db.begin().await.map_err(|_| AuthError::DbTimeout)?;
    let life = lock_lifecycle(&txn).await?;
    let stored = operations::Entity::find_by_id(operation.id)
        .one(&txn)
        .await
        .map_err(|_| AuthError::DbTimeout)?
        .ok_or(AuthError::DbTimeout)?;
    if matches!(
        stored.state,
        OperationState::Applied | OperationState::Failed
    ) {
        return Ok(stored);
    }
    let finished = finish(&txn, &life, stored, enabled, outcome).await?;
    txn.commit().await.map_err(|_| AuthError::DbTimeout)?;
    Ok(finished)
}

/// Converges after restarts and temporary failures, and re-checks the service after a
/// superseded intent. Skips when another process holds the lifecycle.
async fn reconcile(state: &SharedState, wait: LockWait) -> Result<(), AuthError> {
    let access = service_access();
    converge_with(
        &state.database,
        access.as_ref().map(|access| access as &dyn ModelAccess),
        Utc::now(),
        wait,
    )
    .await?;
    refresh_local_engine(state).await.map(|_| ())
}

pub fn spawn_reconciler(state: SharedState) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = reconcile(&state, LockWait::Skip).await {
                eprintln!("Grengin intent reconciliation failed: {error:?}");
            }
            tokio::time::sleep(RECONCILE_EVERY).await;
        }
    });
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
    supersede_open(&txn).await?;
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
        state: match operation.state {
            OperationState::Confirming => OperationState::Pending,
            state => state,
        },
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
    let direct = ai_engines::Entity::find()
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .filter(ai_engines::Column::ApiKey.is_not_null())
        .all(&state.database)
        .await
        .map_err(|_| AuthError::DbTimeout)?;
    let mut models = Vec::with_capacity(listed.len());
    for model in listed {
        let Ok((provider_id, native_model_id)) = split_model(model.id.as_str()) else {
            continue;
        };
        let Ok(own_key) = own_key_covers_engine(
            state,
            direct
                .iter()
                .find(|engine| engine.engine_key == provider_id),
            provider_id,
            native_model_id,
        )
        .await
        else {
            // Never advertise gateway funding when own-key coverage could not be verified.
            eprintln!("Grengin catalog model omitted: own-key coverage unavailable");
            continue;
        };
        models.extend(catalog_entry(&model, engine.as_ref(), own_key));
    }
    Ok(GrenginCatalog {
        observed_at,
        models,
    })
}

fn catalog_entry(
    model: &llm_plugin::ProviderModel,
    engine: Option<&ai_engines::Model>,
    own_key: bool,
) -> Option<GrenginCatalogModel> {
    let id = model.id.as_str().to_owned();
    let (provider_id, native_model_id) = split_model(&id).ok()?;
    let price = |field: &str| model.metadata.get(field).and_then(|value| value.as_i64());
    Some(GrenginCatalogModel {
        provider_id: provider_id.to_owned(),
        native_model_id: native_model_id.to_owned(),
        name: model.name.clone(),
        capability: model_type(model),
        input_per_million_microdollars: price("inputPerMillionMicrodollars"),
        output_per_million_microdollars: price("outputPerMillionMicrodollars"),
        processing_region: None,
        status: "available".into(),
        is_allowed: engine.is_some_and(|engine| engine.whitelist_models.contains(&id)),
        is_default: engine.is_some_and(|engine| {
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
    })
}

/// Safe local and service status, available even when the identity or the local row is missing.
pub async fn status(state: &SharedState) -> Result<GrenginStatus, AuthError> {
    if let Err(error) = reconcile(state, LockWait::Skip).await {
        eprintln!("Grengin intent reconciliation failed: {error:?}");
    }
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

    async fn managed_database() -> DatabaseConnection {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").unwrap();
        let mut options = sea_orm::ConnectOptions::new(url);
        options.max_connections(1);
        let db = sea_orm::Database::connect(options).await.unwrap();
        for table in [
            "ai_engines",
            "grengin_engine_lifecycle",
            "grengin_engine_operations",
        ] {
            db.execute_unprepared(&format!(
                "CREATE TEMP TABLE {table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .await
            .unwrap();
        }
        db
    }

    async fn managed_creator(db: &DatabaseConnection) -> ai_engines::Model {
        let txn = db.begin().await.unwrap();
        preinstall_for_managed_creator(&txn).await.unwrap();
        let initial = ai_engines::Entity::find().one(&txn).await.unwrap().unwrap();
        txn.commit().await.unwrap();
        initial
    }

    struct RegisteredStatus(&'static str);

    #[async_trait::async_trait]
    impl ModelAccess for RegisteredStatus {
        async fn engine_state(&self) -> Result<String, ProviderError> {
            match self.0 {
                "timeout" => Err(ProviderError::Transport("timeout".into())),
                "unauthorized" => Err(ProviderError::HttpStatus {
                    status: 401,
                    message: "unauthorized".into(),
                }),
                "forbidden" => Err(ProviderError::HttpStatus {
                    status: 403,
                    message: "forbidden".into(),
                }),
                "rate_limited" => Err(ProviderError::HttpStatus {
                    status: 429,
                    message: "rate_limited".into(),
                }),
                "unavailable" => Err(ProviderError::HttpStatus {
                    status: 503,
                    message: "unavailable".into(),
                }),
                "mismatch" => Err(ProviderError::Configuration("identity mismatch".into())),
                state => Ok(state.into()),
            }
        }

        async fn set_enabled(&self, _: bool) -> Result<(), ProviderError> {
            panic!("managed registration confirmation must never write consent")
        }
    }

    #[tokio::test]
    async fn managed_confirmation_classifies_permanent_errors_without_writing_consent() {
        for remote in ["unauthorized", "forbidden", "mismatch", "off"] {
            assert_eq!(
                confirm_registration(Some(&RegisteredStatus(remote)))
                    .await
                    .unwrap_err()
                    .0,
                OperationState::Failed
            );
        }
        for remote in ["timeout", "rate_limited", "unavailable", "unknown"] {
            assert_eq!(
                confirm_registration(Some(&RegisteredStatus(remote)))
                    .await
                    .unwrap_err()
                    .0,
                OperationState::Confirming
            );
        }
        assert_eq!(
            confirm_registration(None).await.unwrap_err().0,
            OperationState::Failed
        );
        for remote in ["on", "low", "empty", "pending"] {
            assert!(
                confirm_registration(Some(&RegisteredStatus(remote)))
                    .await
                    .is_ok()
            );
        }
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn managed_confirmation_permanent_denial_is_terminal_across_reconcile_passes() {
        for remote in ["unauthorized", "forbidden", "mismatch"] {
            let db = managed_database().await;
            let txn = db.begin().await.unwrap();
            ai_engines::ActiveModel::native(ENGINE_KEY.into(), "Grengin".into(), false, Utc::now())
                .insert(&txn)
                .await
                .unwrap();
            let life = lock_lifecycle(&txn).await.unwrap();
            record_locked(&txn, life, true, OperationState::Confirming)
                .await
                .unwrap();
            txn.commit().await.unwrap();
            converge_with(
                &db,
                Some(&RegisteredStatus(remote)),
                Utc::now(),
                LockWait::Skip,
            )
            .await
            .unwrap();
            assert_eq!(
                latest_unapplied(&db).await.unwrap().unwrap().state,
                OperationState::Failed
            );
            converge_with(
                &db,
                Some(&RegisteredStatus("on")),
                Utc::now(),
                LockWait::Skip,
            )
            .await
            .unwrap();
            assert_eq!(enabled(&db).await, Some(false));
        }
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn existing_managed_confirmations_recover_without_writing_consent() {
        for remote in ["on", "low", "empty", "pending", "off"] {
            let db = managed_database().await;
            let txn = db.begin().await.unwrap();
            let initial = ai_engines::ActiveModel::native(
                ENGINE_KEY.into(),
                "Grengin".into(),
                false,
                Utc::now(),
            )
            .insert(&txn)
            .await
            .unwrap();
            let life = lock_lifecycle(&txn).await.unwrap();
            record_locked(&txn, life, true, OperationState::Confirming)
                .await
                .unwrap();
            txn.commit().await.unwrap();
            converge_with(
                &db,
                Some(&RegisteredStatus("timeout")),
                Utc::now(),
                LockWait::Skip,
            )
            .await
            .unwrap();
            assert_eq!(enabled(&db).await, Some(false));
            assert_eq!(
                latest_unapplied(&db).await.unwrap().unwrap().state,
                OperationState::Pending
            );

            converge_with(
                &db,
                Some(&RegisteredStatus(remote)),
                Utc::now(),
                LockWait::Skip,
            )
            .await
            .unwrap();
            let row = managed_creator(&db).await;
            assert_eq!(row.id, initial.id);
            assert_eq!(row.is_enabled, remote != "off");
            assert!(row.whitelist_models.is_empty() && row.default_model.is_empty());
            assert!(row.default_image_gen_model.is_none());
            assert_eq!(operations::Entity::find().all(&db).await.unwrap().len(), 1);
            if remote == "off" {
                assert_eq!(
                    latest_unapplied(&db).await.unwrap().unwrap().state,
                    OperationState::Failed
                );
                converge_with(
                    &db,
                    Some(&RegisteredStatus("on")),
                    Utc::now(),
                    LockWait::Skip,
                )
                .await
                .unwrap();
                assert_eq!(enabled(&db).await, Some(false));
            }
        }
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn managed_creator_starts_enabled_and_preserves_policy_disable_and_deletion() {
        let db = managed_database().await;
        let initial = managed_creator(&db).await;
        assert!(crate::services::ai_plugin::is_catalog_grengin(&initial));
        assert_eq!(initial.display_name, "Grengin");
        assert_eq!(initial.api_key_status, ApiKeyStatus::NotConfigured);
        assert!(
            initial.is_enabled
                && initial.whitelist_models.is_empty()
                && initial.default_model.is_empty()
        );
        assert!(
            initial.default_image_gen_model.is_none()
                && initial.base_url_override.is_none()
                && initial.api_key.is_none()
                && initial.api_key_validated_at.is_none()
        );
        assert!(operations::Entity::find().one(&db).await.unwrap().is_none());

        let custom_db = managed_database().await;
        let mut custom = ai_engines::ActiveModel::native(
            ENGINE_KEY.into(),
            "Existing provider".into(),
            false,
            Utc::now(),
        );
        custom.plugin_config = Set(Some(serde_json::json!({"existing": true})));
        let custom = custom.insert(&custom_db).await.unwrap();
        assert_eq!(managed_creator(&custom_db).await, custom);

        let mut edited: ai_engines::ActiveModel = initial.clone().into();
        edited.whitelist_models = Set(vec!["openai/example".into()]);
        let edited = edited.update(&db).await.unwrap();
        assert_eq!(managed_creator(&db).await, edited);
        assert!(operations::Entity::find().one(&db).await.unwrap().is_none());

        record(&db, false).await.unwrap();
        let remote = FakeService::new(true, 1);
        converge(&db, &remote, LockWait::Skip).await;
        assert!(!managed_creator(&db).await.is_enabled);
        assert!(!remote.remote());
        assert_eq!(*remote.sets.lock().unwrap(), vec![false]);

        let db = managed_database().await;
        let initial = managed_creator(&db).await;
        close_installation(&db, initial.id).await.unwrap();
        converge_with(
            &db,
            Some(&RegisteredStatus("on")),
            Utc::now(),
            LockWait::Skip,
        )
        .await
        .unwrap();
        let txn = db.begin().await.unwrap();
        preinstall_for_managed_creator(&txn).await.unwrap();
        assert!(
            ai_engines::Entity::find()
                .one(&txn)
                .await
                .unwrap()
                .is_none()
        );
        txn.rollback().await.unwrap();
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

    #[test]
    fn catalog_entries_show_tariffs_policy_and_who_pays() {
        let model = llm_plugin::ProviderModel {
            id: "openai/gpt-5".into(),
            name: "GPT-5".into(),
            model_type: llm_plugin::ProviderModelType::TextGenerator,
            capabilities: Default::default(),
            metadata: serde_json::json!({
                "inputPerMillionMicrodollars": 1_250_000,
                "outputPerMillionMicrodollars": 10_000_000,
            }),
        };
        let mut engine: ai_engines::Model = serde_json::from_value(serde_json::json!({
            "id": Uuid::new_v4(), "displayName": "Grengin", "engineKey": "grengin",
            "isEnabled": true, "apiKeyStatus": "not_configured", "apiKey": null,
            "apiKeyValidatedAt": null, "pluginConfig": null,
            "whitelistModels": ["openai/gpt-5"], "defaultModel": "openai/gpt-5",
            "defaultImageGenModel": null, "createdAt": Utc::now(), "updatedAt": Utc::now()
        }))
        .unwrap();
        let entry = catalog_entry(&model, Some(&engine), false).unwrap();
        assert_eq!(
            (entry.provider_id.as_str(), entry.native_model_id.as_str()),
            ("openai", "gpt-5")
        );
        assert_eq!(entry.input_per_million_microdollars, Some(1_250_000));
        assert_eq!(entry.output_per_million_microdollars, Some(10_000_000));
        assert!(entry.is_allowed && entry.is_default);
        assert_eq!(entry.effective_engine, "grengin");
        assert!(matches!(
            entry.funding_source,
            GrenginFundingSource::Grengin
        ));

        // An own-key engine that allows the model pays for it; the prices stay Grengin tariffs.
        let own = catalog_entry(&model, Some(&engine), true).unwrap();
        assert_eq!(own.effective_engine, "openai");
        assert!(matches!(own.funding_source, GrenginFundingSource::OwnKey));
        assert_eq!(own.input_per_million_microdollars, Some(1_250_000));

        engine.whitelist_models.clear();
        engine.default_model.clear();
        let excluded = catalog_entry(&model, Some(&engine), false).unwrap();
        assert!(!excluded.is_allowed && !excluded.is_default);
        let uninstalled = catalog_entry(&model, None, false).unwrap();
        assert!(!uninstalled.is_allowed);
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn a_disable_supersedes_a_pending_enable_and_finished_operations_stay_finished() {
        let db = database().await;
        install(&db).await;
        // The enable could not be confirmed: it stays pending and routing stays closed.
        let enable = record(&db, true).await.unwrap();
        let pending = complete(
            &db,
            enable,
            true,
            Err((OperationState::Pending, "service_unavailable".into())),
        )
        .await
        .unwrap();
        assert_eq!(pending.state, OperationState::Pending);
        assert_eq!(enabled(&db).await, Some(false));
        // A disable while the stored flag is already false is still recorded and supersedes it.
        let disable = record(&db, false).await.unwrap();
        let reconciled = complete(&db, pending.clone(), true, Ok(())).await.unwrap();
        assert_eq!(reconciled.error_code.as_deref(), Some("superseded"));
        assert_eq!(enabled(&db).await, Some(false));
        let applied = complete(&db, disable, false, Ok(())).await.unwrap();
        assert_eq!(applied.state, OperationState::Applied);
        // A second finisher (request and reconciler) cannot overwrite the first decision.
        let again = complete(
            &db,
            applied.clone(),
            false,
            Err((OperationState::Pending, "late".into())),
        )
        .await
        .unwrap();
        assert_eq!(again.state, OperationState::Applied);
        assert_eq!(again.error_code, None);
    }

    struct StatusFailure(u16);

    #[async_trait::async_trait]
    impl ModelAccess for StatusFailure {
        async fn engine_state(&self) -> Result<String, ProviderError> {
            Err(ProviderError::HttpStatus {
                status: self.0,
                message: "denied".into(),
            })
        }
        async fn set_enabled(&self, _: bool) -> Result<(), ProviderError> {
            panic!("must not send after a failed status read")
        }
    }

    #[tokio::test]
    async fn permanent_status_errors_fail_and_transient_errors_wait() {
        for status in [400, 401, 403] {
            assert_eq!(
                remote_converge(&StatusFailure(status), true)
                    .await
                    .unwrap_err()
                    .0,
                OperationState::Failed
            );
        }
        for status in [429, 500, 503] {
            assert_eq!(
                remote_converge(&StatusFailure(status), true)
                    .await
                    .unwrap_err()
                    .0,
                OperationState::Pending
            );
        }
    }

    struct StatusAfterSet(std::sync::atomic::AtomicBool);

    #[async_trait::async_trait]
    impl ModelAccess for StatusAfterSet {
        async fn engine_state(&self) -> Result<String, ProviderError> {
            if self.0.swap(false, std::sync::atomic::Ordering::Relaxed) {
                Ok("off".into())
            } else {
                Err(ProviderError::HttpStatus {
                    status: 403,
                    message: "revoked".into(),
                })
            }
        }
        async fn set_enabled(&self, _: bool) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn permanent_confirmation_errors_after_a_set_also_fail() {
        let service = StatusAfterSet(true.into());
        assert_eq!(
            remote_converge(&service, true).await.unwrap_err().0,
            OperationState::Failed
        );
    }

    /// A service whose calls take `delay`; with `lose_next_reply`, the next SET reply is lost
    /// (the caller sees a transport error) while the change still lands `late` afterwards.
    struct FakeService {
        enabled: std::sync::Arc<std::sync::atomic::AtomicBool>,
        delay: std::time::Duration,
        lose_next_reply: std::sync::atomic::AtomicBool,
        late: std::time::Duration,
        sets: std::sync::Mutex<Vec<bool>>,
    }

    impl FakeService {
        fn new(enabled: bool, delay_ms: u64) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(enabled)),
                delay: std::time::Duration::from_millis(delay_ms),
                lose_next_reply: false.into(),
                late: std::time::Duration::from_millis(300),
                sets: Default::default(),
            })
        }
        fn remote(&self) -> bool {
            self.enabled.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl ModelAccess for FakeService {
        async fn engine_state(&self) -> Result<String, ProviderError> {
            Ok(if self.remote() { "on" } else { "off" }.into())
        }
        async fn set_enabled(&self, enabled: bool) -> Result<(), ProviderError> {
            tokio::time::sleep(self.delay).await;
            self.sets.lock().unwrap().push(enabled);
            if self
                .lose_next_reply
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                let remote = self.enabled.clone();
                let late = self.late;
                tokio::spawn(async move {
                    tokio::time::sleep(late).await;
                    remote.store(enabled, std::sync::atomic::Ordering::SeqCst);
                });
                return Err(ProviderError::Transport("reply lost".into()));
            }
            self.enabled
                .store(enabled, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    async fn converge(db: &DatabaseConnection, service: &FakeService, wait: LockWait) {
        converge_with(db, Some(service), Utc::now(), wait)
            .await
            .unwrap();
    }

    // Intent A starts converging with a slow service; intent B is requested during that call.
    async fn concurrent_intents(first: bool, second: bool) {
        let db = database().await;
        install(&db).await;
        let service = FakeService::new(!first, 400);
        let (db_a, service_a) = (db.clone(), service.clone());
        let a = tokio::spawn(async move {
            record(&db_a, first).await.unwrap();
            converge(&db_a, &service_a, LockWait::Wait).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let (db_b, service_b) = (db.clone(), service.clone());
        let b = tokio::spawn(async move {
            record(&db_b, second).await.unwrap();
            converge(&db_b, &service_b, LockWait::Wait).await;
        });
        a.await.unwrap();
        b.await.unwrap();
        // The service and local routing both end on the newest intent.
        assert_eq!(service.remote(), second);
        assert_eq!(enabled(&db).await, Some(second));
        assert_eq!(*service.sets.lock().unwrap(), vec![first, second]);
        // A background pass while another process converges skips instead of waiting.
        converge(&db, &service, LockWait::Skip).await;
        assert_eq!(service.remote(), second);
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn a_newer_enable_during_a_slow_disable_call_wins_at_the_service_and_locally() {
        concurrent_intents(false, true).await;
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn a_newer_disable_during_a_slow_enable_call_wins_at_the_service_and_locally() {
        concurrent_intents(true, false).await;
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn a_lost_call_that_lands_after_a_newer_intent_is_corrected() {
        for (stale, newest) in [(false, true), (true, false)] {
            let db = database().await;
            install(&db).await;
            let service = FakeService::new(newest, 10);
            // The stale intent's reply is lost; its change reaches the service later.
            service
                .lose_next_reply
                .store(true, std::sync::atomic::Ordering::SeqCst);
            record(&db, stale).await.unwrap();
            converge(&db, &service, LockWait::Wait).await;
            // The newest intent sees the service still matching and is applied at once.
            record(&db, newest).await.unwrap();
            converge(&db, &service, LockWait::Wait).await;
            assert_eq!(enabled(&db).await, Some(newest));
            // Then the stale change lands and contradicts the newest intent.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            assert_eq!(service.remote(), stale);
            // The next reconciliation pass re-checks and converges the service to it.
            converge(&db, &service, LockWait::Skip).await;
            assert_eq!(service.remote(), newest);
            assert_eq!(enabled(&db).await, Some(newest));
            // Outside the re-check window an applied intent is left alone.
            converge_with(
                &db,
                Some(service.as_ref()),
                Utc::now() + VERIFY_AFTER_SUPERSEDED + chrono::Duration::minutes(1),
                LockWait::Skip,
            )
            .await
            .unwrap();
            let sets = service.sets.lock().unwrap().len();
            service
                .enabled
                .store(stale, std::sync::atomic::Ordering::SeqCst);
            converge_with(
                &db,
                Some(service.as_ref()),
                Utc::now() + VERIFY_AFTER_SUPERSEDED + chrono::Duration::minutes(1),
                LockWait::Skip,
            )
            .await
            .unwrap();
            assert_eq!(service.sets.lock().unwrap().len(), sets);
        }
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn a_lost_call_from_before_a_reinstall_is_corrected_for_the_new_installation() {
        let db = database().await;
        let engine = install(&db).await;
        let service = FakeService::new(true, 10);
        // A disable whose reply is lost; its change reaches the service later.
        service
            .lose_next_reply
            .store(true, std::sync::atomic::Ordering::SeqCst);
        record(&db, false).await.unwrap();
        converge(&db, &service, LockWait::Wait).await;
        // The engine is deleted and installed again: a new local generation, same service.
        close_installation(&db, engine).await.unwrap();
        install(&db).await;
        record(&db, true).await.unwrap();
        converge(&db, &service, LockWait::Wait).await;
        assert_eq!(enabled(&db).await, Some(true));
        // The old disable lands and turns the service off behind the new installation.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert!(!service.remote());
        converge(&db, &service, LockWait::Skip).await;
        assert!(service.remote());
        assert_eq!(enabled(&db).await, Some(true));
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn an_open_intent_is_finished_by_the_next_pass_after_a_failed_call() {
        let db = database().await;
        install(&db).await;
        let service = FakeService::new(false, 10);
        service
            .lose_next_reply
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let operation = record(&db, true).await.unwrap();
        converge(&db, &service, LockWait::Wait).await;
        let stored = operations::Entity::find_by_id(operation.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, OperationState::Pending);
        assert_eq!(enabled(&db).await, Some(false));
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        // Restart or timer: the service already has it, so no second SET is sent.
        converge(&db, &service, LockWait::Skip).await;
        let stored = operations::Entity::find_by_id(operation.id)
            .one(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, OperationState::Applied);
        assert_eq!(enabled(&db).await, Some(true));
        assert_eq!(service.sets.lock().unwrap().len(), 1);
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
