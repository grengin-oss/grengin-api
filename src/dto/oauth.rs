use serde::Deserialize;
use utoipa::{ToSchema};

#[derive(Deserialize,ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OidcProvider{
  Azure,
  Google
}

#[derive(Deserialize)]
pub struct StartParams {
   pub redirect_to: Option<String>,
}

#[derive(Deserialize)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
}