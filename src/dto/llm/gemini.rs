use serde::{Deserialize, Serialize};
use crate::{llm::prompt::Prompt, models::messages::ChatRole};

pub const GEMINI_DEFAULT_MAX_TOKENS: i32 = 8192;

// ============== Request Structures ==============

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerateContentRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum GeminiPart {
    Text { text: String },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
}

// ============== Response Structures ==============

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiGenerateContentResponse {
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default)]
    pub usage_metadata: Option<GeminiUsageMetadata>,
    #[serde(default)]
    pub model_version: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiCandidate {
    pub content: Option<GeminiContent>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub safety_ratings: Option<Vec<GeminiSafetyRating>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiSafetyRating {
    pub category: String,
    pub probability: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUsageMetadata {
    #[serde(default)]
    pub prompt_token_count: Option<i32>,
    #[serde(default)]
    pub candidates_token_count: Option<i32>,
    #[serde(default)]
    pub total_token_count: Option<i32>,
}

// ============== Streaming Response Structures ==============

/// Streaming response from Gemini (same structure as non-streaming but partial)
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiStreamChunk {
    pub candidates: Option<Vec<GeminiCandidate>>,
    #[serde(default)]
    pub usage_metadata: Option<GeminiUsageMetadata>,
    #[serde(default)]
    pub model_version: Option<String>,
}

// ============== Conversion Methods ==============

impl GeminiContent {
    pub fn new_text(role: Option<&str>, text: String) -> Self {
        Self {
            role: role.map(|r| r.to_string()),
            parts: vec![GeminiPart::Text { text }],
        }
    }

    pub fn from_prompts(prompts: Vec<Prompt>) -> (Vec<Self>, Option<Self>) {
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for prompt in prompts {
            match prompt.role {
                ChatRole::System => {
                    system_instruction = Some(GeminiContent::new_text(None, prompt.text));
                }
                ChatRole::User => {
                    contents.push(GeminiContent::new_text(Some("user"), prompt.text));
                }
                ChatRole::Assistant => {
                    contents.push(GeminiContent::new_text(Some("model"), prompt.text));
                }
                ChatRole::Tool => {
                    // Gemini handles tool results differently, for now treat as user
                    contents.push(GeminiContent::new_text(Some("user"), prompt.text));
                }
            }
        }

        (contents, system_instruction)
    }
}

impl GeminiGenerationConfig {
    pub fn new(temperature: Option<f32>, max_output_tokens: Option<i32>) -> Self {
        Self {
            temperature,
            top_p: None,
            top_k: None,
            max_output_tokens,
            stop_sequences: None,
        }
    }
}

impl GeminiStreamChunk {
    /// Extract text content from the streaming chunk
    pub fn get_text(&self) -> Option<String> {
        self.candidates
            .as_ref()?
            .first()?
            .content
            .as_ref()?
            .parts
            .first()
            .and_then(|part| match part {
                GeminiPart::Text { text } => Some(text.clone()),
            })
    }
}
