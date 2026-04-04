use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use crate::models::users::UserStatus;

#[derive(Serialize, ToSchema)]
pub struct User {
    pub id: Uuid,
    pub sub: String,
    #[schema(format = "email")]
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(format = "uri")]
    pub picture: Option<String>,
    /// Hosted domain (organization domain)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hd: Option<String>,
    pub roles: Vec<String>,
    pub status:UserStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    pub department_id: Option<Uuid>,
    #[serde(default)]
    pub is_super_admin: bool,
    pub has_password: bool,
    pub mfa_enabled: bool,
    pub last_login_at: Option<DateTime<Utc>>,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Object)]
    pub effective_permissions: Option<serde_json::Value>,
}

#[derive(Serialize,ToSchema)]
pub struct PaginatedUsers{
  pub users:Vec<User>,
  pub total:u64,
  pub limit:u64,
  pub offset:u64
}

#[derive(Deserialize,ToSchema)]
pub struct UserCreate{
   pub email:String,
   pub name:String,
   pub department_id:Option<Uuid>,
}


#[derive(Deserialize,ToSchema)]
pub struct UserUpdate{
   pub email:Option<String>,
   pub name:Option<String>,
   pub department_id:Option<Uuid>,
   /// When true, clear any existing department assignment.
   pub unassign_department: Option<bool>,
}

#[derive(Deserialize,ToSchema)]
pub struct UserPatchRequest{
   pub status:UserStatus,
}
