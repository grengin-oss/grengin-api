// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, Set};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{auth::error::AuthError, models::auth_audit_events};

pub fn build_audit_payload<T: serde::Serialize>(value: T) -> Option<Value> {
    serde_json::to_value(value)
        .map_err(|e| {
            eprintln!("audit payload error: {e}");
        })
        .ok()
}

pub async fn record_auth_event<C: ConnectionTrait>(
    db: &C,
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

    active.insert(db).await.map_err(|e| {
        eprintln!("audit insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok(())
}

pub async fn record_required_auth_event<C: ConnectionTrait, T: Serialize>(
    db: &C,
    event: &str,
    actor_id: Option<Uuid>,
    payload: T,
) -> Result<(), AuthError> {
    let payload = required_audit_payload(payload)?;
    record_auth_event(db, event, actor_id, payload).await
}

fn required_audit_payload<T: Serialize>(payload: T) -> Result<Value, AuthError> {
    serde_json::to_value(payload).map_err(|e| {
        eprintln!("audit payload error: {e}");
        AuthError::ServiceTemporarilyUnavailable
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::admin_roles::RoleAssignmentPayload;
    use serde::ser::Error as _;

    struct FailingPayload;

    impl Serialize for FailingPayload {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(S::Error::custom("serialization failed"))
        }
    }

    #[test]
    fn required_payload_preserves_scoped_and_unscoped_grants() {
        let assignment_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let role_id = Uuid::new_v4();
        let scope_department_id = Uuid::new_v4();
        for scope in [Some(scope_department_id), None] {
            let payload = required_audit_payload(RoleAssignmentPayload {
                assignment_id,
                user_id,
                role_id,
                scope_department_id: scope,
            })
            .unwrap();
            assert_eq!(payload["assignment_id"], assignment_id.to_string());
            assert_eq!(payload["user_id"], user_id.to_string());
            assert_eq!(payload["role_id"], role_id.to_string());
            assert_eq!(payload["scope_department_id"], serde_json::json!(scope));
        }
    }

    #[test]
    fn required_payload_rejects_serialization_failure() {
        assert!(matches!(
            required_audit_payload(FailingPayload),
            Err(AuthError::ServiceTemporarilyUnavailable)
        ));
    }
}
