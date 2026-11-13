// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::EntityTrait;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::users::{self, UserStatus},
    services::{
        chat_stream_helpers::{DepartmentBudgetGate, department_budget_status},
        department_policies::check_model_allowed,
    },
    state::SharedState,
};

// Background LLM work billed to a user must respect that user's status, department budget
// and department model policy, the same as their chat requests.
pub async fn ensure_user_llm_allowed(
    state: &SharedState,
    user_id: Uuid,
    provider: &str,
    model: &str,
) -> Result<(), AppError> {
    let user = users::Entity::find_by_id(user_id)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbTimeout)?
        .ok_or(AppError::ResourceNotFound)?;
    if user.status != UserStatus::Active {
        return Err(AppError::ResourceNotFound);
    }
    if let Some(department) = user.department_id {
        let (gate, _, _) = department_budget_status(&state.database, department).await?;
        if gate == DepartmentBudgetGate::Block {
            return Err(AppError::DepartmentBudgetExceeded);
        }
    }
    if !check_model_allowed(&state.database, user.department_id, provider, model)
        .await
        .map_err(|_| AppError::DbTimeout)?
    {
        return Err(AppError::DepartmentModelNotAllowed {
            provider: provider.to_string(),
            model: model.to_string(),
        });
    }
    Ok(())
}
