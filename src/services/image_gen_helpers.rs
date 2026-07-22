use anyhow::{anyhow, Context};
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
) -> anyhow::Result<(Uuid, String, i32, i32, i32)> {
    let input_images = load_input_images(app_state, input_file_ids).await?;

    let result = match provider {
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
                .openai_generate_image(&settings, model, prompt, &input_images, None, None)
                .await
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
                .gemini_generate_image(&settings, model, prompt, &input_images)
                .await
        }
        other => Err(anyhow!("unsupported image gen provider: {other}")),
    }?;

    let file_id = Uuid::new_v4();
    let ext = if result.content_type == "image/png" { "png" } else { "webp" };
    let filename = format!("{file_id}.{ext}");
    let dir = format!("{LOCAL_FOLDER}/{user_id}/images/{file_id}");

    fs::create_dir_all(&dir).context("create image dir")?;

    let local_path = format!("{dir}/{filename}");
    let mut f = fs::File::create(&local_path).context("create image file")?;
    f.write_all(&result.bytes).context("write image file")?;

    let now = Utc::now();
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

    Ok((file_id, result.content_type, result.text_input_tokens, result.image_input_tokens, result.output_tokens))
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
        let bytes = fs::read(&file.local_path)
            .with_context(|| format!("read input image {id}"))?;
        images.push(InputImage { bytes, content_type: file.content_type });
    }
    Ok(images)
}
