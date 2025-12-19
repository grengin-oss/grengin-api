use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelsResponse {
    pub providers: Vec<ProviderInfo>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub key: String,
    pub name: String,
    pub icon: String,
    pub status: String,
    pub models: Vec<ModelInfo>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub key: String,
    pub name: String,
    pub engine: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_pdf_native: bool,
    pub supports_web_search: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_images: Option<i32>,
}