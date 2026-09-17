// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::{Context, anyhow};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::json;
use std::{fs, io::Write};
use uuid::Uuid;

use crate::{
    handlers::file::LOCAL_FOLDER,
    models::{ai_engines, files::{self, FileUploadStatus}},
    state::SharedState,
};

/// Name the LLM sees and calls; must match the `name` field the provider echoes
/// back on a tool call, so the chat_stream dispatch loop can recognize it before
/// it ever reaches MCP-specific dispatch (which assumes a real `server_id`).
pub const IMAGE_GENERATION_TOOL_NAME: &str = "generate_image";

pub fn image_generation_tool_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "prompt": {
                "type": "string",
                "description": "A detailed, self-contained description of the image to generate."
            },
            "count": {
                "type": "integer",
                "description": "How many image variations to generate, if the model supports more than one.",
                "minimum": 1,
                "maximum": 4
            }
        },
        "required": ["prompt"]
    })
}

/// Resolves which (engine_key, model) services the image-generation tool call.
/// Four combinations, all valid:
/// - provider + model: look up that exact engine, verify it's enabled and the
///   model is whitelisted on it. No scan, no ambiguity if the same model id
///   ever ends up whitelisted on more than one engine.
/// - model only: scan enabled engines for one whitelisting that model (the
///   prior behavior, kept for callers that only know the model).
/// - provider only: use that engine's own `defaultImageGenModel`, if set.
/// - neither: auto-select the first enabled engine with `defaultImageGenModel`
///   configured, so the tool still works without the caller naming anything.
pub async fn resolve_image_tool_target(
    db: &DatabaseConnection,
    requested_provider: Option<&str>,
    requested_model: Option<&str>,
) -> anyhow::Result<Option<(String, String)>> {
    if let Some(provider) = requested_provider {
        let Some(engine) = ai_engines::Entity::find()
            .filter(ai_engines::Column::EngineKey.eq(provider))
            .filter(ai_engines::Column::IsEnabled.eq(true))
            .one(db)
            .await
            .context("load the requested image engine")?
        else {
            return Ok(None);
        };
        return Ok(match requested_model {
            Some(model) if engine.whitelist_models.iter().any(|id| id == model) => {
                Some((engine.engine_key, model.to_string()))
            }
            Some(_) => None,
            None => engine
                .default_image_gen_model
                .clone()
                .map(|model| (engine.engine_key, model)),
        });
    }

    if let Some(model) = requested_model {
        let engines = ai_engines::Entity::find()
            .filter(ai_engines::Column::IsEnabled.eq(true))
            .all(db)
            .await
            .context("load enabled AI engines to resolve the requested image model")?;
        return Ok(engines
            .into_iter()
            .find(|engine| engine.whitelist_models.iter().any(|id| id == model))
            .map(|engine| (engine.engine_key, model.to_string())));
    }

    let engines = ai_engines::Entity::find()
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .filter(ai_engines::Column::DefaultImageGenModel.is_not_null())
        .all(db)
        .await
        .context("load enabled AI engines to auto-select an image model")?;
    Ok(engines.into_iter().find_map(|engine| {
        engine
            .default_image_gen_model
            .clone()
            .map(|model| (engine.engine_key, model))
    }))
}

pub async fn generate_and_save(
    app_state: &SharedState,
    user_id: Uuid,
    provider: &dyn llm_plugin::ProviderPlugin,
    provider_key: &str,
    model: &str,
    prompt: &str,
    input_file_ids: &[Uuid],
    count: u8,
) -> anyhow::Result<Vec<(Uuid, String, i32, i32, i32)>> {
    let input_images = load_input_images(app_state, user_id, input_file_ids).await?;
    let generator = provider
        .images()
        .ok_or_else(|| anyhow!("provider does not support image generation: {provider_key}"))?;
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
                "provider image request failed ({})",
                crate::services::provider_chat::provider_error_class(&error)
            )
        })?;
    let usage = response.usage.unwrap_or_default();
    let image_count = response.images.len();
    let total_input_tokens = usage.input_tokens.and_then(to_i32).unwrap_or(0);
    let image_input_tokens = usage.image_input_tokens.and_then(to_i32).unwrap_or(0);
    let text_input_tokens = usage
        .text_input_tokens
        .and_then(to_i32)
        .unwrap_or_else(|| total_input_tokens.saturating_sub(image_input_tokens));
    let output_tokens = usage.output_tokens.and_then(to_i32).unwrap_or(0);
    let results = response
        .images
        .into_iter()
        .enumerate()
        .map(|(index, image)| GeneratedImageForStorage {
            bytes: image.bytes,
            content_type: image.media_type,
            text_input_tokens: distributed_usage(text_input_tokens, index, image_count),
            image_input_tokens: distributed_usage(image_input_tokens, index, image_count),
            output_tokens: distributed_usage(output_tokens, index, image_count),
        })
        .collect::<Vec<_>>();

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
                "provider": provider_key,
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

async fn load_input_images(
    app_state: &SharedState,
    user_id: Uuid,
    file_ids: &[Uuid],
) -> anyhow::Result<Vec<llm_plugin::InputImage>> {
    let mut images = Vec::with_capacity(file_ids.len());
    for &id in file_ids {
        let file = files::Entity::find_by_id(id)
            .one(&app_state.database)
            .await
            .context("db lookup input image")?
            .ok_or_else(|| anyhow!("input image file not found: {id}"))?;
        if file.user_id != user_id {
            return Err(anyhow!("input image file not found: {id}"));
        }
        let bytes = fs::read(&file.local_path).with_context(|| format!("read input image {id}"))?;
        images.push(llm_plugin::InputImage {
            data: BASE64.encode(bytes),
            media_type: file.content_type,
            filename: Some(file.name),
        });
    }
    Ok(images)
}

fn to_i32(value: u32) -> Option<i32> {
    i32::try_from(value).ok()
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
