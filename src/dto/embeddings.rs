use serde::Deserialize;

// Response envelope from https://meta.grengin.com/providers/{provider}/text_embedders.json
#[derive(Debug, Deserialize)]
pub struct EmbeddersResponse {
    pub models: Vec<EmbedderModelMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbedderModelMeta {
    pub id: String,
    pub dimensions: usize,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct MistralEmbeddingResponse {
    pub data: Vec<MistralEmbeddingData>,
}

#[derive(Debug, Deserialize)]
pub struct MistralEmbeddingData {
    pub embedding: Vec<f32>,
    pub index: usize,
}

#[derive(Debug, Deserialize)]
pub struct GeminiEmbeddingResponse {
    pub embedding: Option<GeminiEmbeddingData>,
}

#[derive(Debug, Deserialize)]
pub struct GeminiEmbeddingData {
    pub values: Vec<f32>,
}
