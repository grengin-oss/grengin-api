// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::departments::{ActionOnExceed, BudgetPeriod};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema)]
pub struct SubDepartmentBudgetDto {
    pub department_id: Uuid,
    pub name: String,
    pub allocated: f32,
    pub used: f32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DepartmentBudgetStatus {
    pub department_id: Uuid,
    pub budget_allocated: f32,
    pub budget_distributed: f32,
    pub budget_available: f32,
    pub budget_used: f32,
    pub budget_used_total: f32,
    pub period: BudgetPeriod,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub sub_department_budgets: Vec<SubDepartmentBudgetDto>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SetDepartmentBudgetRequest {
    pub budget_allocated: f32,
    pub budget_period: BudgetPeriod,
    pub action_on_exceed: ActionOnExceed,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DepartmentBudgetUpdatedDto {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
    pub path: String,
    pub depth: i32,
    pub budget_allocated: f32,
    pub budget_distributed: f32,
    pub budget_available: f32,
    pub budget_used: f32,
    pub budget_period: BudgetPeriod,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
