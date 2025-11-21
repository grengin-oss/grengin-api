use serde::{Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize,ToSchema)]
#[serde(rename_all="lowercase")]
pub enum LoginType{
    Individual,
    Organization
}

#[derive(Serialize,IntoParams,ToSchema)]
#[serde(rename_all="camelCase")]
pub struct LoginResponse {
    pub token:String,
    pub login_type:LoginType,
}