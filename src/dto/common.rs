use serde::Deserialize;
use utoipa::ToSchema;
use crate::models::users::{UserRole, UserStatus};

#[derive(Debug,Deserialize,ToSchema)]
#[serde(rename_all="lowercase")]
pub enum SortRule {
    Name,
    Email,
    CreatedAt,
    UpdatedAt,
    LastLoginAt,
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
}