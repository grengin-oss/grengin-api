use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::sea_query::{Expr, ExprTrait, Func};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::PERMISSION_AI_PLATFORM_MANAGE,
    },
    dto::prompts::{
        DepartmentPromptAssignmentCreate, DepartmentPromptAssignmentListQuery,
        DepartmentPromptAssignmentResponse, DepartmentPromptAssignmentUpdate, PromptMetricRow,
        PromptMetricsQuery, PromptMetricsResponse, RolePromptCreate, RolePromptListQuery,
        RolePromptResponse, RolePromptUpdate, to_assignment_response, to_role_prompt_response,
    },
    models::{
        department_prompt_assignments, departments, prompt_feedback, role_prompts, roles,
        user_prompt_preferences,
    },
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};

#[utoipa::path(
    get,
    path = "/admin/role-prompts",
    tag = "admin",
    params(RolePromptListQuery),
    responses(
        (status = 200, body = [RolePromptResponse]),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn list_role_prompts(
    claims: Claims,
    Query(query): Query<RolePromptListQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<RolePromptResponse>>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let mut select = role_prompts::Entity::find();
    if let Some(role_id) = query.role_id {
        select = select.filter(role_prompts::Column::RoleId.eq(role_id));
    }
    if let Some(is_system) = query.is_system {
        select = select.filter(role_prompts::Column::IsSystem.eq(is_system));
    }

    let rows = select
        .order_by_desc(role_prompts::Column::UpdatedAt)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role prompts list error: {e}");
            AuthError::DbTimeout
        })?;

    let payload = rows.into_iter().map(to_role_prompt_response).collect();
    Ok((StatusCode::OK, Json(payload)))
}

#[utoipa::path(
    get,
    path = "/admin/role-prompts/{prompt_id}",
    tag = "admin",
    params(("prompt_id" = Uuid, Path, description = "Prompt id")),
    responses(
        (status = 200, body = RolePromptResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_role_prompt(
    claims: Claims,
    Path(prompt_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<RolePromptResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let model = role_prompts::Entity::find_by_id(prompt_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role prompt lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    Ok((StatusCode::OK, Json(to_role_prompt_response(model))))
}

#[utoipa::path(
    post,
    path = "/admin/role-prompts",
    tag = "admin",
    request_body = RolePromptCreate,
    responses(
        (status = 201, body = RolePromptResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn create_role_prompt(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<RolePromptCreate>,
) -> Result<(StatusCode, Json<RolePromptResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let variables = req.variables.clone().map(|vars| {
        serde_json::Value::Array(vars.into_iter().map(serde_json::Value::String).collect())
    });

    let role_exists = roles::Entity::find_by_id(req.role_id)
        .select_only()
        .column(roles::Column::Id)
        .into_tuple::<Uuid>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role lookup error: {e}");
            AuthError::DbTimeout
        })?
        .is_some();
    if !role_exists {
        return Err(AuthError::ResourceNotFound);
    }

    let model = role_prompts::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(req.name),
        role_id: Set(req.role_id),
        prompt_text: Set(req.prompt_text),
        variables: Set(variables),
        is_system: Set(req.is_system),
        created_by: Set(Some(claims.user_id)),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        usage_count: Set(0),
    };

    let model = model.insert(&app_state.database).await.map_err(|e| {
        eprintln!("role prompt insert error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::CREATED, Json(to_role_prompt_response(model))))
}

#[utoipa::path(
    put,
    path = "/admin/role-prompts/{prompt_id}",
    tag = "admin",
    request_body = RolePromptUpdate,
    params(("prompt_id" = Uuid, Path, description = "Prompt id")),
    responses(
        (status = 200, body = RolePromptResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn update_role_prompt(
    claims: Claims,
    Path(prompt_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(req): Json<RolePromptUpdate>,
) -> Result<(StatusCode, Json<RolePromptResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let model = role_prompts::Entity::find_by_id(prompt_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role prompt lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let mut active = model.into_active_model();
    if let Some(name) = req.name {
        active.name = Set(name);
    }
    if let Some(role_id) = req.role_id {
        let role_exists = roles::Entity::find_by_id(role_id)
            .select_only()
            .column(roles::Column::Id)
            .into_tuple::<Uuid>()
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?
            .is_some();
        if !role_exists {
            return Err(AuthError::ResourceNotFound);
        }
        active.role_id = Set(role_id);
    }
    if let Some(prompt_text) = req.prompt_text {
        active.prompt_text = Set(prompt_text);
    }
    if let Some(vars) = req.variables {
        let value =
            serde_json::Value::Array(vars.into_iter().map(serde_json::Value::String).collect());
        active.variables = Set(Some(value));
    }
    if let Some(is_system) = req.is_system {
        active.is_system = Set(is_system);
    }
    active.updated_at = Set(Utc::now());

    let model = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("role prompt update error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(to_role_prompt_response(model))))
}

#[utoipa::path(
    delete,
    path = "/admin/role-prompts/{prompt_id}",
    tag = "admin",
    params(("prompt_id" = Uuid, Path, description = "Prompt id")),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn delete_role_prompt(
    claims: Claims,
    Path(prompt_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let assignments_count = department_prompt_assignments::Entity::find()
        .filter(department_prompt_assignments::Column::PromptId.eq(prompt_id))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department prompt assignments lookup error: {e}");
            AuthError::DbTimeout
        })?;

    let user_prefs_count = user_prompt_preferences::Entity::find()
        .filter(user_prompt_preferences::Column::PromptId.eq(prompt_id))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("user prompt preferences lookup error: {e}");
            AuthError::DbTimeout
        })?;

    if assignments_count > 0 || user_prefs_count > 0 {
        return Err(AuthError::DbConflict);
    }

    role_prompts::Entity::delete_by_id(prompt_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("role prompt delete error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/admin/department-prompts",
    tag = "admin",
    params(DepartmentPromptAssignmentListQuery),
    responses(
        (status = 200, body = [DepartmentPromptAssignmentResponse]),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn list_department_prompts(
    claims: Claims,
    Query(query): Query<DepartmentPromptAssignmentListQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<DepartmentPromptAssignmentResponse>>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let mut select = department_prompt_assignments::Entity::find();
    if let Some(department_id) = query.department_id {
        select =
            select.filter(department_prompt_assignments::Column::DepartmentId.eq(department_id));
    }

    let rows = select
        .order_by_asc(department_prompt_assignments::Column::Priority)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department prompt list error: {e}");
            AuthError::DbTimeout
        })?;

    Ok((
        StatusCode::OK,
        Json(rows.into_iter().map(to_assignment_response).collect()),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/department-prompts",
    tag = "admin",
    request_body = DepartmentPromptAssignmentCreate,
    responses(
        (status = 201, body = DepartmentPromptAssignmentResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 409, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn assign_department_prompt(
    claims: Claims,
    State(app_state): State<SharedState>,
    Json(req): Json<DepartmentPromptAssignmentCreate>,
) -> Result<(StatusCode, Json<DepartmentPromptAssignmentResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let dept_exists = departments::Entity::find_by_id(req.department_id)
        .select_only()
        .column(departments::Column::Id)
        .into_tuple::<Uuid>()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department lookup error: {e}");
            AuthError::DbTimeout
        })?
        .is_some();
    if !dept_exists {
        return Err(AuthError::ResourceNotFound);
    }

    let prompt_exists = role_prompts::Entity::find_by_id(req.prompt_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("prompt lookup error: {e}");
            AuthError::DbTimeout
        })?
        .is_some();
    if !prompt_exists {
        return Err(AuthError::ResourceNotFound);
    }

    let model = department_prompt_assignments::ActiveModel {
        id: Set(Uuid::new_v4()),
        department_id: Set(req.department_id),
        prompt_id: Set(req.prompt_id),
        priority: Set(req.priority),
        assigned_by: Set(Some(claims.user_id)),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    };

    let model = model.insert(&app_state.database).await.map_err(|e| {
        eprintln!("department prompt insert error: {e}");
        if e.to_string().contains("uniq_department_prompt") {
            AuthError::DbConflict
        } else {
            AuthError::DbTimeout
        }
    })?;

    Ok((StatusCode::CREATED, Json(to_assignment_response(model))))
}

#[utoipa::path(
    put,
    path = "/admin/department-prompts/{assignment_id}",
    tag = "admin",
    request_body = DepartmentPromptAssignmentUpdate,
    params(("assignment_id" = Uuid, Path, description = "Assignment id")),
    responses(
        (status = 200, body = DepartmentPromptAssignmentResponse),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn update_department_prompt(
    claims: Claims,
    Path(assignment_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(req): Json<DepartmentPromptAssignmentUpdate>,
) -> Result<(StatusCode, Json<DepartmentPromptAssignmentResponse>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let assignment = department_prompt_assignments::Entity::find_by_id(assignment_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department prompt lookup error: {e}");
            AuthError::DbTimeout
        })?
        .ok_or(AuthError::ResourceNotFound)?;

    let mut active = assignment.into_active_model();
    active.priority = Set(req.priority);
    active.updated_at = Set(Utc::now());

    let model = active.update(&app_state.database).await.map_err(|e| {
        eprintln!("department prompt update error: {e}");
        AuthError::DbTimeout
    })?;

    Ok((StatusCode::OK, Json(to_assignment_response(model))))
}

#[utoipa::path(
    delete,
    path = "/admin/department-prompts/{assignment_id}",
    tag = "admin",
    params(("assignment_id" = Uuid, Path, description = "Assignment id")),
    responses(
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn delete_department_prompt(
    claims: Claims,
    Path(assignment_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    department_prompt_assignments::Entity::delete_by_id(assignment_id)
        .exec(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("department prompt delete error: {e}");
            AuthError::DbTimeout
        })?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/admin/prompt-metrics",
    tag = "admin",
    params(PromptMetricsQuery),
    responses(
        (status = 200, body = [PromptMetricsResponse]),
        (status = 401, content_type = "application/json", body = Error),
        (status = 403, content_type = "application/json", body = Error),
        (status = 503, content_type = "application/json", body = Error),
    )
)]
pub async fn get_prompt_metrics(
    claims: Claims,
    Query(query): Query<PromptMetricsQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<Vec<PromptMetricsResponse>>), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_AI_PLATFORM_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let average_rating_expr = Func::avg(Expr::col((
        prompt_feedback::Entity,
        prompt_feedback::Column::Rating,
    )))
    .cast_as("double precision");
    let mut select = role_prompts::Entity::find()
        .select_only()
        .column_as(role_prompts::Column::Id, "promptId")
        .column_as(role_prompts::Column::Name, "name")
        .column_as(role_prompts::Column::RoleId, "roleId")
        .column_as(role_prompts::Column::UsageCount, "usageCount")
        .expr_as(
            Func::count(Expr::col((
                prompt_feedback::Entity,
                prompt_feedback::Column::Id,
            ))),
            "feedbackCount",
        )
        .expr_as(average_rating_expr, "averageRating")
        .left_join(prompt_feedback::Entity)
        .group_by(role_prompts::Column::Id);

    if let Some(prompt_id) = query.prompt_id {
        select = select.filter(role_prompts::Column::Id.eq(prompt_id));
    }
    if let Some(role_id) = query.role_id {
        select = select.filter(role_prompts::Column::RoleId.eq(role_id));
    }

    let rows = select
        .into_model::<PromptMetricRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("prompt metrics query error: {e}");
            AuthError::DbTimeout
        })?;

    let payload = rows
        .into_iter()
        .map(|row| PromptMetricsResponse {
            prompt_id: row.prompt_id,
            name: row.name,
            role_id: row.role_id,
            usage_count: row.usage_count,
            feedback_count: row.feedback_count,
            average_rating: row.average_rating,
        })
        .collect();

    Ok((StatusCode::OK, Json(payload)))
}
