use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
pub struct ModelsResponse {
    pub providers: Vec<ProviderInfo>,
}

impl ModelsResponse {
    pub fn get_icons<S: Into<String>>(&self, provider: S) -> (Option<String>, Option<String>) {
        let provider_key = provider.into();
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.key == provider_key);
        let icon = provider.map(|provider| provider.icon.clone());
        let icon_dark = provider.map(|provider| provider.icon_dark.clone());
        (icon, icon_dark)
    }

    pub fn default() -> Self {
        ModelsResponse {
            providers: Vec::new(),
        }
    }
}

#[derive(Serialize, Clone, ToSchema)]
pub struct ProviderInfo {
    pub key: String,
    pub name: String,
    pub icon: String,
    pub icon_dark: String,
    pub status: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Serialize, Clone, ToSchema)]
pub struct ModelInfo {
    pub key: String,
    pub name: String,
    pub engine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_token_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_token_rate: Option<f64>,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_pdf_native: bool,
    pub supports_web_search: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_images: Option<i32>,
}
