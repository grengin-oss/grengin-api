// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Error, anyhow};
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use reqwest::{Client as ReqwestClient, multipart};
use serde::{Deserialize, Serialize};

use crate::{
    config::setting::OpenaiSettings,
    image_gen::provider::{ImageGenResult, InputImage, OpenaiImageGenApis},
    llm::openai::OPENAI_API_URL,
};

#[derive(Serialize)]
struct TextToImageRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    n: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quality: Option<&'a str>,
}

#[derive(Deserialize)]
struct ImageGenResponse {
    data: Vec<ImageDataItem>,
    usage: Option<ImageGenUsage>,
}

#[derive(Deserialize)]
struct ImageDataItem {
    b64_json: Option<String>,
}

#[derive(Deserialize)]
struct ImageGenUsage {
    input_tokens: i32,
    input_tokens_details: Option<InputTokensDetails>,
    output_tokens: i32,
}

#[derive(Deserialize)]
struct InputTokensDetails {
    text_tokens: i32,
    image_tokens: i32,
}

#[async_trait]
impl OpenaiImageGenApis for ReqwestClient {
    async fn openai_generate_image(
        &self,
        settings: &OpenaiSettings,
        model: &str,
        prompt: &str,
        input_images: &[InputImage],
        size: Option<&str>,
        quality: Option<&str>,
        count: u8,
    ) -> Result<Vec<ImageGenResult>, Error> {
        if input_images.is_empty() {
            let body = TextToImageRequest { model, prompt, n: count, size, quality };
            let resp = self
                .post(format!("{OPENAI_API_URL}/v1/images/generations"))
                .bearer_auth(&settings.api_key)
                .json(&body)
                .send()
                .await?;
            extract_all(resp).await
        } else {
            let mut form = multipart::Form::new()
                .text("model", model.to_string())
                .text("prompt", prompt.to_string())
                .text("n", count.to_string());
            if let Some(s) = size { form = form.text("size", s.to_string()); }
            if let Some(q) = quality { form = form.text("quality", q.to_string()); }
            for img in input_images {
                let part = multipart::Part::bytes(img.bytes.clone())
                    .mime_str(&img.content_type)?
                    .file_name("image.png");
                form = form.part("image[]", part);
            }
            let resp = self
                .post(format!("{OPENAI_API_URL}/v1/images/edits"))
                .bearer_auth(&settings.api_key)
                .multipart(form)
                .send()
                .await?;
            extract_all(resp).await
        }
    }
}

async fn extract_all(resp: reqwest::Response) -> Result<Vec<ImageGenResult>, Error> {
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("openai image gen {status}: {body}"));
    }
    let parsed: ImageGenResponse = resp.json().await?;
    let (text_input_tokens, image_input_tokens, output_tokens) = parsed
        .usage
        .map(|u| {
            let text = u.input_tokens_details.as_ref().map(|d| d.text_tokens).unwrap_or(u.input_tokens);
            let image = u.input_tokens_details.as_ref().map(|d| d.image_tokens).unwrap_or(0);
            (text, image, u.output_tokens)
        })
        .unwrap_or((0, 0, 0));
    let n = parsed.data.len().max(1) as i32;
    parsed
        .data
        .into_iter()
        .map(|item| {
            let b64 = item.b64_json.ok_or_else(|| anyhow!("openai image response missing b64_json field"))?;
            Ok(ImageGenResult {
                bytes: B64.decode(&b64)?,
                content_type: "image/png".to_string(),
                // tokens are reported once for the whole request; split evenly per image
                text_input_tokens: text_input_tokens / n,
                image_input_tokens: image_input_tokens / n,
                output_tokens: output_tokens / n,
            })
        })
        .collect()
}
