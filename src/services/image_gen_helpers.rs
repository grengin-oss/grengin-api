// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Context, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use serde_json::json;
use std::{fs, io::Write};
use uuid::Uuid;

use crate::{
    handlers::file::LOCAL_FOLDER,
    image_gen::provider::{GeminiImageGenApis, InputImage, OpenaiImageGenApis},
    models::files::{self, FileUploadStatus},
    state::SharedState,
};

pub async fn generate_and_save(
    app_state: &SharedState,
    user_id: Uuid,
    provider: &str,
    model: &str,
    prompt: &str,
    input_file_ids: &[Uuid],
    count: u8,
) -> anyhow::Result<Vec<(Uuid, String, i32, i32, i32)>> {
    let input_images = load_input_images(app_state, input_file_ids).await?;

    let provider_key = provider.to_lowercase();
    let results: Vec<GeneratedImageForStorage> = match provider_key.as_str() {
        "openai" => {
            let settings = app_state
                .settings
                .openai
                .read()
                .await
                .clone()
                .ok_or_else(|| anyhow!("openai not configured"))?;
            app_state
                .req_client
                .openai_generate_image(&settings, model, prompt, &input_images, None, None, count)
                .await?
                .into_iter()
                .map(GeneratedImageForStorage::from)
                .collect()
        }
        "gemini" => {
            let settings = app_state
                .settings
                .gemini
                .read()
                .await
                .clone()
                .ok_or_else(|| anyhow!("gemini not configured"))?;
            app_state
                .req_client
                .gemini_generate_image(&settings, model, prompt, &input_images, count)
                .await?
                .into_iter()
                .map(GeneratedImageForStorage::from)
                .collect()
        }
        other => {
            let plugin = app_state
                .provider_registry
                .get_by_str(other)
                .await
                .ok_or_else(|| anyhow!("unsupported image gen provider: {other}"))?;
            let generator = plugin
                .images()
                .ok_or_else(|| anyhow!("provider does not support image generation: {other}"))?;
            let input_images = input_images
                .into_iter()
                .map(|image| llm_plugin::InputImage {
                    data: BASE64.encode(image.bytes),
                    media_type: image.content_type,
                    filename: None,
                })
                .collect();
            let response = generator
                .generate(llm_plugin::ImageRequest {
                    model: llm_plugin::ModelId::new(model),
                    prompt: prompt.to_string(),
                    input_images,
                    count,
                    size: None,
                    quality: None,
                    options: serde_json::Value::Null,
                })
                .await
                .map_err(|error| {
                    anyhow!(
                        "provider plugin image request failed ({})",
                        crate::services::provider_chat::provider_error_class(&error)
                    )
                })?;
            let usage = response.usage.unwrap_or_default();
            let image_count = response.images.len();
            let input_tokens = usage
                .input_tokens
                .and_then(|tokens| i32::try_from(tokens).ok())
                .unwrap_or(0);
            let output_tokens = usage
                .output_tokens
                .and_then(|tokens| i32::try_from(tokens).ok())
                .unwrap_or(0);
            response
                .images
                .into_iter()
                .enumerate()
                .map(|(index, image)| GeneratedImageForStorage {
                    bytes: image.bytes,
                    content_type: image.media_type,
                    text_input_tokens: distributed_usage(input_tokens, index, image_count),
                    image_input_tokens: 0,
                    output_tokens: distributed_usage(output_tokens, index, image_count),
                })
                .collect()
        }
    };

    let mut saved = Vec::with_capacity(results.len());
    let now = Utc::now();
    for result in results {
        let file_id = Uuid::new_v4();
        let ext = if result.content_type == "image/png" {
            "png"
        } else {
            "webp"
        };
        let filename = format!("{file_id}.{ext}");
        let dir = format!("{LOCAL_FOLDER}/{user_id}/images/{file_id}");

        fs::create_dir_all(&dir).context("create image dir")?;

        let local_path = format!("{dir}/{filename}");
        let mut f = fs::File::create(&local_path).context("create image file")?;
        f.write_all(&result.bytes).context("write image file")?;

        let active = files::ActiveModel {
            id: Set(file_id),
            user_id: Set(user_id),
            name: Set(filename),
            content_type: Set(result.content_type.clone()),
            size: Set(result.bytes.len() as i64),
            local_path: Set(local_path),
            description: Set(None),
            url: Set(None),
            status: Set(FileUploadStatus::Uploaded),
            created_at: Set(now),
            updated_at: Set(now),
            metadata: Set(Some(json!({
                "prompt": prompt,
                "model": model,
                "provider": provider,
            }))),
        };
        active
            .insert(&app_state.database)
            .await
            .context("db insert image file")?;

        saved.push((
            file_id,
            result.content_type,
            result.text_input_tokens,
            result.image_input_tokens,
            result.output_tokens,
        ));
    }
    Ok(saved)
}

fn distributed_usage(total: i32, index: usize, item_count: usize) -> i32 {
    let Ok(item_count) = i32::try_from(item_count) else {
        return 0;
    };
    if item_count == 0 {
        return 0;
    }
    let remainder = total % item_count;
    total / item_count + i32::from(i32::try_from(index).is_ok_and(|index| index < remainder))
}

struct GeneratedImageForStorage {
    bytes: Vec<u8>,
    content_type: String,
    text_input_tokens: i32,
    image_input_tokens: i32,
    output_tokens: i32,
}

impl From<crate::image_gen::provider::ImageGenResult> for GeneratedImageForStorage {
    fn from(image: crate::image_gen::provider::ImageGenResult) -> Self {
        Self {
            bytes: image.bytes,
            content_type: image.content_type,
            text_input_tokens: image.text_input_tokens,
            image_input_tokens: image.image_input_tokens,
            output_tokens: image.output_tokens,
        }
    }
}

async fn load_input_images(
    app_state: &SharedState,
    file_ids: &[Uuid],
) -> anyhow::Result<Vec<InputImage>> {
    let mut images = Vec::with_capacity(file_ids.len());
    for &id in file_ids {
        let file = files::Entity::find_by_id(id)
            .one(&app_state.database)
            .await
            .context("db lookup input image")?
            .ok_or_else(|| anyhow!("input image file not found: {id}"))?;
        let bytes = fs::read(&file.local_path).with_context(|| format!("read input image {id}"))?;
        images.push(InputImage {
            bytes,
            content_type: file.content_type,
        });
    }
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::distributed_usage;

    #[test]
    fn usage_is_distributed_without_changing_the_total() {
        let shares = (0..3)
            .map(|index| distributed_usage(8, index, 3))
            .collect::<Vec<_>>();

        assert_eq!(shares, vec![3, 3, 2]);
        assert_eq!(shares.into_iter().sum::<i32>(), 8);
    }

    #[test]
    fn empty_result_sets_do_not_divide_by_zero() {
        assert_eq!(distributed_usage(8, 0, 0), 0);
    }
}
