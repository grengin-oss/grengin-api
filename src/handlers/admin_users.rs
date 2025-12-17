use axum::{Json, extract::{Path, Query, State}};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, sea_query::OnConflict};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::AuthError}, dto::{admin_user::{UserDetails, UserRequest, UserResponse, UserUpdateRequest}, common::{PaginationQuery, SortRule}}, models::users::{self, UserRole, UserStatus}, state::SharedState};

#[utoipa::path(
    get,
    path = "/admin/users/{user_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    responses(
        (status = 200, body = UserDetails),
        (status = 204, description = "User deleted"),
        (status = 404, description = "Resource not found"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_user_by_id(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
) -> Result<(StatusCode,Json<UserDetails>), AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
    let user = users::Entity::find_by_id(user_id)
      .one(&app_state.database)
      .await
          .map_err(|e| {
        eprintln!("insert error: {e}");
        AuthError::ServiceTemporarilyUnavailable
       })?
       .ok_or(AuthError::EmailDoesNotExist)?;
     let user_response = UserDetails {
         id: user.id,
         org_id: user.org_id,
         sub: user.google_id.unwrap_or(user.azure_id.unwrap_or(user.email.clone())),
         email: user.email,
         name: user.name,
         picture: user.picture,
         hd: user.hd,
         role: user.role, // TODO: Map from database if role field exists
         status: user.status,
         department: user.department,
         is_super_admin: user.role == UserRole::SuperAdmin, // Default to false, update based on database field if available
         has_password: user.password.is_some(), // SSO-only users don't have password
         mfa_enabled: user.mfa_enabled,
         last_login_at: Some(user.last_login_at),
         password_changed_at: None,
         created_at: user.created_at,
         updated_at: user.updated_at,
      };
    Ok((StatusCode::OK,Json(user_response)))
}

#[utoipa::path(
    get,
    path = "/admin/users",
    tag = "admin",
    params(
        ("limit" = Option<u64>, Query, description = "Default value : 20"),
        ("offset" = Option<u64>, Query, description = "Default value : 0"),
        ("search" = Option<String>, Query, description = "Search by name"),
        ("department" = Option<String>, Query, description = "Search by department"),
        ("status" = Option<UserStatus>, Query, description = "Account status"),
        ("sort" = Option<SortRule>, Query, description = "Sorting param"),
    ),
    responses(
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
    )
)]
pub async fn get_users(
  claims:Claims,
  Query(query):Query<PaginationQuery>,
  State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<UserResponse>),AuthError>{
   match claims.role {
      UserRole::SuperAdmin | UserRole::Admin => (),
      _ => return Err(AuthError::PermissionDenied)
   }
   let mut response = UserResponse { 
     users:Vec::new(),
     total:0,limit:query.limit.unwrap_or(20),
     offset:query.offset.unwrap_or(0) 
   };
   let mut select = users::Entity::find()
     .offset(query.offset.unwrap_or(0))
     .limit(query.limit.unwrap_or(20));
   if let Some(role) = query.role  {
       select = select.filter(users::Column::Role.eq(role));
   }
   if let Some(department) = query.department{
       select = select.filter(users::Column::Department.contains(department))
   }
   if let Some(status) = query.status{
       select = select.filter(users::Column::Status.eq(status))
   }
   if let Some(sort) = query.sort{
       select = match sort {
          SortRule::Name => select.order_by_desc(users::Column::Name),
          SortRule::Email => select.order_by_desc(users::Column::Email),
          SortRule::CreatedAt => select.order_by_desc(users::Column::CreatedAt),
          SortRule::UpdatedAt => select.order_by_desc(users::Column::UpdatedAt),
          SortRule::LastLoginAt => select.order_by_asc(users::Column::LastLoginAt),
      };
   }
   let rows = select
      .all(&app_state.database)
      .await
      .map_err(|e|{
         eprintln!("Db get many error: {}",e);
         AuthError::ServiceTemporarilyUnavailable
     })?;
     response.users = rows
       .into_iter()
       .map(|user| UserDetails {
         id: user.id,
         org_id:user.org_id,
         sub: user.google_id.unwrap_or(user.azure_id.unwrap_or(user.email.clone())),
         email: user.email,
         name: user.name,
         picture: user.picture,
         hd: user.hd,
         role: user.role, // TODO: Map from database if role field exists
         status: user.status,
         department: user.department,
         is_super_admin: user.role == UserRole::SuperAdmin, // Default to false, update based on database field if available
         has_password: user.password.is_some(), // SSO-only users don't have password
         mfa_enabled: user.mfa_enabled,
         last_login_at: Some(user.last_login_at),
         password_changed_at: None,
         created_at: user.created_at,
         updated_at: user.updated_at,
      }).collect();
     response.total = response.users.len() as u64;
 Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    post,
    path = "/admin/users",
    tag = "admin",
    request_body = UserRequest,
    responses(
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 409, description = "Email already registered"),
    )
)]
pub async fn add_new_user(
  claims:Claims,
  State(app_state): State<SharedState>,
  Json(req):Json<UserRequest>
) -> Result<StatusCode,AuthError>{
    match claims.role {
      UserRole::SuperAdmin | UserRole::Admin => (),
      _ => return Err(AuthError::PermissionDenied)
   }
   let user = users::ActiveModel{ 
     id: Set(Uuid::new_v4()),
     org_id:Set(claims.org_id),
     status: Set(UserStatus::Active),
     picture: Set(None),
     email: Set(req.email.trim().to_string()),
     email_verified: Set(false),
     name: Set(Some(req.name)),
     password: Set(None),
     google_id: Set(None),
     azure_id: Set(None),
     mfa_enabled: Set(false),
     mfa_secret: Set(None),
     created_at: Set(Utc::now()),
     updated_at: Set(Utc::now()),
     last_login_at: Set(Utc::now()),
     password_changed_at: Set(None),
     role: Set(req.role),
     hd:Set(req.email.trim().split("@").collect::<Vec<_>>().last().map(|t| t.to_string())),
     department:Set(Some(req.department)),
     metadata:Set(None), 
    };
   let affected: u64 = users::Entity::insert(user)
    .on_conflict(
        OnConflict::column(users::Column::Email)
            .do_nothing()
            .to_owned(),
    )
    .exec_without_returning(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("insert error: {e}");
        AuthError::ServiceTemporarilyUnavailable
    })?;

   if affected == 0 {
     return Err(AuthError::EmailAlreadyExist);
   }
 Ok(StatusCode::CREATED)
}

#[utoipa::path(
    put,
    path = "/admin/users/{user_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    request_body = UserUpdateRequest,
    responses(
        (status = 200, description = "User updated"),
        (status = 404, description = "Email does not exist"),
        (status = 409, description = "Email already registered"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn update_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UserUpdateRequest>,
) -> Result<StatusCode, AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
    let model = users::Entity::find_by_id(user_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?
        .ok_or(AuthError::EmailDoesNotExist)?;

    let mut active: users::ActiveModel = model.into();

    if let Some(email) = req.email {
        let email = email.trim().to_string();
        active.email = Set(email.clone());
        active.hd = Set(email.split('@').nth(1).map(|s| s.to_string()));
        active.email_verified = Set(false);
    }
    if let Some(name) = req.name {
        active.name = Set(Some(name));
    }
    if let Some(role) = req.role {
        active.role = Set(role);
    }
    if let Some(dept) = req.department {
        active.department = Set(Some(dept));
    }
    if let Some(status) = req.status{
        active.status = Set(status);
    }
    active.updated_at = Set(Utc::now());
    active
        .update(&app_state.database)
        .await
        .map_err(|e| {
            let s = e.to_string();
            if s.contains("23505") || s.contains("duplicate key value violates unique constraint") {
                AuthError::EmailAlreadyExist
            } else {
                eprintln!("db update error: {e}");
                AuthError::ServiceTemporarilyUnavailable
            }
        })?;

    Ok(StatusCode::OK)
}

#[utoipa::path(
    delete,
    path = "/admin/users/{user_id}",
    tag = "admin",
    params(
        ("user_id" = Uuid, Path, description = "User id")
    ),
    responses(
        (status = 204, description = "User deleted"),
        (status = 404, description = "Resource not found"),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn delete_user(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, AuthError> {
    match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
    }
    
    let user = users::Entity::find_by_id(user_id)
      .one(&app_state.database)
      .await
      .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?
      .ok_or(AuthError::EmailDoesNotExist)?;
     let mut active_model =user
       .into_active_model();
     active_model.updated_at = Set(Utc::now());
     active_model.status = Set(UserStatus::Deleted);
     active_model
       .update(&app_state.database)
       .await
       .map_err(|e| {
            eprintln!("db find error: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    Ok(StatusCode::NO_CONTENT)
}