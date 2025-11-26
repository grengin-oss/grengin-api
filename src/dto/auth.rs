use serde::{Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Serialize,IntoParams,ToSchema)]
#[serde(rename_all="camelCase")]
pub struct LoginResponse {
    pub token:String,
}