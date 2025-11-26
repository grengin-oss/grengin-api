use serde::Deserialize;

// Free-form string identifier for auth providers (e.g., "google", "azure", "keycloak", "authentik", "okta")
// The provider serves as a URL slug and display name. Actual OIDC behavior is determined by configuration.
pub type AuthProvider = String;

#[derive(Deserialize)]
pub struct StartParams {
   pub redirect_uri: Option<String>,
}

#[derive(Deserialize)]
pub struct OAuthCallback {
    pub code: Option<String>,
    pub state: String,
    pub error: Option<String>,
    pub error_description: Option<String>,
}