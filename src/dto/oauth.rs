use serde::Deserialize;

#[derive(Deserialize)]
pub struct StartParams {
   pub redirect_to: Option<String>,
}

#[derive(Deserialize)]
pub struct OAuthCallback {
    pub code: String,
    pub state: String,
}