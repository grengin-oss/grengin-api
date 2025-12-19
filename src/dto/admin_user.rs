use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use crate::{models::users::{UserRole, UserStatus}};

#[derive(Serialize, ToSchema)]
pub struct UserDetails {
    pub id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id:Option<Uuid>,
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
    pub role:UserRole,
    pub status:UserStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(default)]
    pub is_super_admin: bool,
    pub has_password: bool,
    pub mfa_enabled: bool,
    pub last_login_at: Option<DateTime<Utc>>,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize,ToSchema)]
pub struct UserResponse{
  pub users:Vec<UserDetails>,
  pub total:u64,
  pub limit:u64,
  pub offset:u64
}

#[derive(Deserialize,ToSchema)]
pub struct UserRequest{
   pub email:String,
   pub name:String,
   pub role:UserRole,
   pub department:String,
}


#[derive(Deserialize,ToSchema)]
pub struct UserUpdateRequest{
   pub email:Option<String>,
   pub name:Option<String>,
   pub role:Option<UserRole>,
   pub department:Option<String>,
   pub status:Option<UserStatus>,
}