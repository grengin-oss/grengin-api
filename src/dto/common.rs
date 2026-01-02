use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::models::users::{UserRole, UserStatus};

#[derive(Debug,Deserialize,ToSchema)]
#[serde(rename_all="snake_case")]
pub enum SortRule {
    Name,
    Email,
    CreatedAt,
    UpdatedAt,
    LastLoginAt,
    Size
}

#[derive(Debug,Deserialize,ToSchema)]
pub struct PaginationQuery {
   pub limit:Option<u64>,
   pub offset:Option<u64>,
   pub search:Option<String>,
   pub archived:Option<bool>,
   pub ascending:Option<bool>,
   pub role:Option<UserRole>,
   pub status:Option<UserStatus>,
   pub department:Option<String>,
   pub sort:Option<SortRule>,
   #[serde(rename = "type")]
   pub content_type:Option<String>
}

#[derive(Serialize,ToSchema)]
pub struct PaginatedResponse<T>{
    #[serde(skip_serializing_if = "Option::is_none")]
   pub users:Option<Vec<T>>,
   pub total:u64,
   pub limit:u64,
   pub offset:u64,
    #[serde(skip_serializing_if = "Option::is_none")]
   pub files:Option<Vec<T>>,
}