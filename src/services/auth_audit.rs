use chrono::Utc;
use sea_orm::{ActiveModelTrait, Set};
use serde_json::Value;
use uuid::Uuid;

use crate::{auth::error::AuthError, models::auth_audit_events};

pub async fn record_auth_event(
    db: &sea_orm::DatabaseConnection,
    event: &str,
    actor_id: Option<Uuid>,
    payload: Value,
) -> Result<(), AuthError> {
    let active = auth_audit_events::ActiveModel {
        id: Set(Uuid::new_v4()),
        event: Set(event.to_string()),
        actor_id: Set(actor_id),
        payload: Set(Some(payload)),
        created_at: Set(Utc::now()),
    };

    active
        .insert(db)
        .await
        .map_err(|e| {
            eprintln!("audit insert error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(())
}
