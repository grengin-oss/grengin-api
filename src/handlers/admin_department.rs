use axum::{Json, extract::State};
use reqwest::StatusCode;

use crate::{auth::{claims::Claims, error::AuthError}, dto::{admin_department::{DepartmentResponse}}, models::users::UserRole, state::SharedState};

#[utoipa::path(
    get,
    path = "/admin/department",
    tag = "admin",
    responses(
        (status = 200, body = DepartmentResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    )
)]
pub async fn get_departments(
    claims: Claims,
     State(app_state): State<SharedState>,
) -> Result<(StatusCode,Json<DepartmentResponse>), AuthError> {
     match claims.role {
        UserRole::SuperAdmin | UserRole::Admin => {}
        _ => return Err(AuthError::PermissionDenied),
      }
      let response = DepartmentResponse{
         departments:Vec::new()
      };       
  Ok((StatusCode::OK,Json(response)))
}
