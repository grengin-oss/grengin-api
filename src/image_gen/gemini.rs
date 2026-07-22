use anyhow::{Error, anyhow};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::Client as ReqwestClient;
use serde::{Deserialize, Serialize};

use crate::{
    config::setting::GeminiSettings,
    image_gen::provider::{GeminiImageGenApis, ImageGenResult, InputImage},
    llm::gemini::GEMINI_API_URL,
};

#[derive(Serialize)]
struct ImageGenRequest {
    contents: Vec<Content>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct Content {
    parts: Vec<Part>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Part {
    Text { text: String },
    Inline { #[serde(rename = "inlineData")] inline_data: InlineInput },
}

#[derive(Serialize)]
struct InlineInput {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Serialize)]
struct GenerationConfig {
    #[serde(rename = "responseModalities")]
    response_modalities: Vec<&'static str>,
}

#[derive(Deserialize)]
struct ImageGenResponse {
    candidates: Vec<Candidate>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<UsageMetadata>,
}

#[derive(Deserialize)]
struct UsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<i32>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<i32>,
}

#[derive(Deserialize)]
struct Candidate {
    content: CandidateContent,
}

#[derive(Deserialize)]
struct CandidateContent {
    parts: Vec<CandidatePart>,
}

#[derive(Deserialize)]
struct CandidatePart {
    #[serde(rename = "inlineData")]
    inline_data: Option<InlineData>,
}

#[derive(Deserialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[async_trait]
impl GeminiImageGenApis for ReqwestClient {
    async fn gemini_generate_image(
        &self,
        settings: &GeminiSettings,
        model: &str,
        prompt: &str,
        input_images: &[InputImage],
    ) -> Result<ImageGenResult, Error> {
        let mut parts = input_images
            .iter()
            .map(|img| Part::Inline {
                inline_data: InlineInput {
                    mime_type: img.content_type.clone(),
                    data: B64.encode(&img.bytes),
                },
            })
            .collect::<Vec<_>>();
        parts.push(Part::Text { text: prompt.to_string() });

        let body = ImageGenRequest {
            contents: vec![Content { parts }],
            generation_config: GenerationConfig {
                response_modalities: vec!["IMAGE", "TEXT"],
            },
        };

        let url = format!(
            "{GEMINI_API_URL}/v1beta/models/{model}:generateContent?key={}",
            settings.api_key
        );

        let resp = self.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("gemini image gen {status}: {body}"));
        }

        let parsed: ImageGenResponse = resp.json().await?;

        let input_tokens = parsed
            .usage_metadata
            .as_ref()
            .and_then(|u| u.prompt_token_count)
            .unwrap_or(0);
        let output_tokens = parsed
            .usage_metadata
            .as_ref()
            .and_then(|u| u.candidates_token_count)
            .unwrap_or(0);

        for part in parsed
            .candidates
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("gemini response has no candidates"))?
            .content
            .parts
        {
            if let Some(inline) = part.inline_data {
                return Ok(ImageGenResult {
                    bytes: B64.decode(&inline.data)?,
                    content_type: inline.mime_type,
                    text_input_tokens: input_tokens,
                    image_input_tokens: 0,
                    output_tokens,
                });
            }
        }

        Err(anyhow!("gemini response contains no image data"))
    }
}
