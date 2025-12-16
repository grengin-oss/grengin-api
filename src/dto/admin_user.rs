use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use crate::{dto::auth::User, models::users::UserRole};

#[derive(Serialize,ToSchema)]
pub struct UserResponse{
  pub users:Vec<User>,
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
}