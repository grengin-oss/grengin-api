use base64::prelude::*;
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use tokio::fs;
use uuid::Uuid;

use crate::{
    config::setting::EmbeddingSettings,
    dto::llm::anthropic::{
        AnthropicContentBlock, AnthropicDocSource, AnthropicImageSource, AnthropicMessage,
        AnthropicRole,
    },
    error::AppError,
    llm::provider::AnthropicApis,
    models::{files, project_source_chunks, project_sources, project_sources::ProcessingStatus},
    services::rag::{format_pgvector, generate_embeddings},
    state::SharedState,
};

pub async fn update_source_status(
    db: &DatabaseConnection,
    source_id: Uuid,
    status: ProcessingStatus,
    error: Option<&str>,
) -> Result<(), AppError> {
    use sea_orm::sea_query::Expr;
    project_sources::Entity::update_many()
        .col_expr(
            project_sources::Column::ProcessingStatus,
            Expr::value(status.to_string()),
        )
        .col_expr(
            project_sources::Column::ProcessingError,
            Expr::value(error.map(|s| s.to_string())),
        )
        .filter(project_sources::Column::Id.eq(source_id))
        .exec(db)
        .await
        .map_err(|e| {
            eprintln!("status update error: {e}");
            AppError::DbTimeout
        })?;
    Ok(())
}

pub fn chunk_text(text: &str) -> Vec<String> {
    const CHUNK_SIZE: usize = 1200;
    const OVERLAP: usize = 150;

    if text.is_empty() {
        return vec![];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + CHUNK_SIZE).min(chars.len());
        let chunk: String = chars[start..end].iter().collect();
        let chunk = chunk.trim().to_string();
        if !chunk.is_empty() {
            chunks.push(chunk);
        }
        if end == chars.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP);
    }
    chunks
}

fn extract_html_text(bytes: &[u8]) -> String {
    let html = String::from_utf8_lossy(bytes);
    scraper::Html::parse_document(&html)
        .root_element()
        .text()
        .map(|t| t.trim())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_csv_text(bytes: &[u8]) -> String {
    let mut reader = csv::ReaderBuilder::new().flexible(true).from_reader(bytes);
    let mut lines = Vec::new();
    if let Ok(headers) = reader.headers().cloned() {
        lines.push(headers.iter().collect::<Vec<_>>().join(", "));
    }
    for record in reader.records().flatten() {
        let row = record.iter().collect::<Vec<_>>().join(", ");
        if !row.trim().is_empty() {
            lines.push(row);
        }
    }
    lines.join("\n")
}

fn extract_spreadsheet_text(bytes: &[u8]) -> Result<String, String> {
    use calamine::{Data, Reader, open_workbook_auto_from_rs};
    use std::io::Cursor;

    let cursor = Cursor::new(bytes);
    let mut workbook = open_workbook_auto_from_rs(cursor).map_err(|e| e.to_string())?;
    let sheet_names = workbook.sheet_names().to_vec();
    let mut all_text = Vec::new();

    for name in &sheet_names {
        let Ok(range) = workbook.worksheet_range(name) else { continue };
        let mut sheet_lines = Vec::new();
        for row in range.rows() {
            let cells: Vec<String> = row.iter().map(|c| match c {
                Data::Empty => String::new(),
                other => other.to_string(),
            }).collect();
            let line = cells.join("\t");
            if !line.trim().is_empty() {
                sheet_lines.push(line);
            }
        }
        if !sheet_lines.is_empty() {
            all_text.push(format!("# {name}\n{}", sheet_lines.join("\n")));
        }
    }

    Ok(all_text.join("\n\n"))
}

fn extract_pdf_text(bytes: &[u8]) -> Option<String> {
    match pdf_extract::extract_text_from_mem(bytes) {
        Ok(text) => {
            let trimmed = text.trim().to_string();
            // Below 50 chars is likely a scan-only PDF with no extractable text
            if trimmed.len() >= 50 { Some(trimmed) } else { None }
        }
        Err(e) => {
            eprintln!("pdf-extract error: {e}");
            None
        }
    }
}

fn effective_mime(bytes: &[u8], declared: &str) -> String {
    if declared == "application/octet-stream" || declared.is_empty() {
        if let Some(kind) = infer::get(bytes) {
            return kind.mime_type().to_string();
        }
    }
    declared.to_string()
}

async fn extract_text_via_llm(
    app_state: &SharedState,
    bytes: Vec<u8>,
    content_type: &str,
) -> Result<String, AppError> {
    let anthropic_settings = app_state
        .settings
        .anthropic
        .read()
        .await
        .clone()
        .ok_or_else(|| AppError::LlmProviderNotConfigured {
            provider: "anthropic".to_string(),
        })?;

    let data = BASE64_STANDARD.encode(&bytes);
    let prompt_text = if content_type.starts_with("image/") {
        "Describe the content of this image in detail. If it contains text, extract all the text verbatim. If it's a diagram or chart, describe what it shows."
    } else {
        "Extract and return all the text content from this document. Preserve the structure where possible."
    };

    let content_block = if content_type == "application/pdf" {
        AnthropicContentBlock::Document {
            source: AnthropicDocSource {
                source_type: "base64".to_string(),
                media_type: "application/pdf".to_string(),
                data: Some(data),
                url: None,
            },
        }
    } else {
        AnthropicContentBlock::Image {
            source: AnthropicImageSource {
                source_type: "base64".to_string(),
                media_type: Some(content_type.to_string()),
                data: Some(data),
                url: None,
            },
        }
    };

    let messages = vec![AnthropicMessage::with_blocks(
        AnthropicRole::User,
        vec![
            content_block,
            AnthropicContentBlock::Text { text: prompt_text.to_string() },
        ],
    )];

    let result = app_state
        .req_client
        .anthropic_generate_text(
            &anthropic_settings,
            "claude-haiku-4-5".to_string(),
            2048,
            messages,
            None,
            None,
        )
        .await
        .map_err(|e| {
            eprintln!("llm text extraction error: {e}");
            AppError::LlmProviderNotConfigured { provider: "anthropic".to_string() }
        })?;

    Ok(result.text)
}

async fn extract_text_from_file(
    app_state: &SharedState,
    local_path: &str,
    content_type: &str,
) -> Result<String, AppError> {
    let bytes = fs::read(local_path).await.map_err(|e| {
        eprintln!("file read error {local_path}: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let mime = effective_mime(&bytes, content_type);
    let text = match mime.as_str() {
        "text/html" | "application/xhtml+xml" => extract_html_text(&bytes),
        "text/plain" | "text/markdown" | "text/x-markdown" => {
            String::from_utf8_lossy(&bytes).to_string()
        }
        "text/csv" | "application/csv" => extract_csv_text(&bytes),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.ms-excel"
        | "application/vnd.ms-excel.sheet.macroEnabled.12"
        | "application/vnd.oasis.opendocument.spreadsheet" => {
            extract_spreadsheet_text(&bytes).unwrap_or_else(|e| {
                eprintln!("spreadsheet extraction failed: {e}");
                String::new()
            })
        }
        "application/pdf" => match extract_pdf_text(&bytes) {
            Some(text) => text,
            None => extract_text_via_llm(app_state, bytes, "application/pdf").await?,
        },
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "application/msword"
        | "application/vnd.ms-powerpoint" => {
            extract_text_via_llm(app_state, bytes, &mime).await?
        }
        m if m.starts_with("image/") => extract_text_via_llm(app_state, bytes, &mime).await?,
        _ => String::from_utf8_lossy(&bytes).to_string(),
    };

    Ok(text)
}

pub async fn delete_source_chunks(db: &DatabaseConnection, source_id: Uuid) -> Result<(), AppError> {
    project_source_chunks::Entity::delete_many()
        .filter(project_source_chunks::Column::ProjectSourceId.eq(source_id))
        .exec(db)
        .await
        .map_err(|e| {
            eprintln!("chunk delete error: {e}");
            AppError::DbTimeout
        })?;
    Ok(())
}

async fn store_chunks(
    db: &DatabaseConnection,
    source_id: Uuid,
    project_id: Uuid,
    chunks: &[String],
    embeddings: &[Vec<f32>],
    config: &EmbeddingSettings,
) -> Result<(), AppError> {
    delete_source_chunks(db, source_id).await?;

    let now = Utc::now();
    for (i, (chunk, embedding)) in chunks.iter().zip(embeddings.iter()).enumerate() {
        // $9::vector cast required — SeaORM has no pgvector type and no crate integration exists.
        let sql = r#"
            INSERT INTO "project_source_chunks"
                ("id", "projectSourceId", "projectId", "chunkIndex", "content",
                 "provider", "model", "dimensions", "embedding", "createdAt")
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::vector, $10)
            ON CONFLICT ("projectSourceId", "chunkIndex") DO NOTHING
        "#;
        db.execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            vec![
                Uuid::new_v4().into(),
                source_id.into(),
                project_id.into(),
                (i as i32).into(),
                chunk.clone().into(),
                config.provider.clone().into(),
                config.model.clone().into(),
                config.dimensions.into(),
                format_pgvector(embedding).into(),
                now.into(),
            ],
        ))
        .await
        .map_err(|e| {
            eprintln!("chunk insert error: {e}");
            AppError::DbTimeout
        })?;
    }
    Ok(())
}

pub async fn process_project_source(
    app_state: SharedState,
    source_id: Uuid,
    project_id: Uuid,
    file_id: Uuid,
) {
    let db = &app_state.database;

    if let Err(e) = update_source_status(db, source_id, ProcessingStatus::Processing, None).await {
        eprintln!("processing status update failed: {e:?}");
        return;
    }

    let result = async {
        let file = files::Entity::find_by_id(file_id)
            .one(db)
            .await
            .map_err(|_| AppError::DbTimeout)?
            .ok_or(AppError::ResourceNotFound)?;

        let text = extract_text_from_file(&app_state, &file.local_path, &file.content_type).await?;

        let embedding_config = app_state
            .settings
            .get_embedding_config()
            .await
            .ok_or(AppError::ServiceTemporarilyUnavailable)?;

        if !embedding_config.is_enabled {
            return Ok(());
        }

        let chunks = chunk_text(&text);
        if chunks.is_empty() {
            return Ok(());
        }

        let embeddings = generate_embeddings(&app_state, &embedding_config, chunks.clone())
            .await?
            .ok_or(AppError::ServiceTemporarilyUnavailable)?;

        store_chunks(db, source_id, project_id, &chunks, &embeddings, &embedding_config).await
    }
    .await;

    match result {
        Ok(()) => {
            let _ = update_source_status(db, source_id, ProcessingStatus::Ready, None).await;
        }
        Err(e) => {
            let msg = format!("{e:?}");
            let _ = update_source_status(db, source_id, ProcessingStatus::Error, Some(&msg)).await;
        }
    }
}

pub fn spawn_process_source(app_state: SharedState, source_id: Uuid, project_id: Uuid, file_id: Uuid) {
    tokio::spawn(async move {
        process_project_source(app_state, source_id, project_id, file_id).await;
    });
}

pub fn build_file_path(user_id: Uuid, file_uuid: Uuid, filename: &str) -> String {
    format!("/data/files/{user_id}/file/{file_uuid}/{filename}")
}

pub async fn write_artifact_file(
    db: &DatabaseConnection,
    user_id: Uuid,
    filename: &str,
    content_type: &str,
    content: &str,
) -> Result<(Uuid, String), AppError> {
    let file_uuid = Uuid::new_v4();
    let local_path = build_file_path(user_id, file_uuid, filename);

    if let Some(parent) = std::path::Path::new(&local_path).parent() {
        fs::create_dir_all(parent).await.map_err(|e| {
            eprintln!("mkdir error: {e}");
            AppError::ServiceTemporarilyUnavailable
        })?;
    }

    fs::write(&local_path, content.as_bytes()).await.map_err(|e| {
        eprintln!("file write error: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let now = Utc::now();
    let file_row = files::ActiveModel {
        id: Set(file_uuid),
        user_id: Set(user_id),
        name: Set(filename.to_string()),
        content_type: Set(content_type.to_string()),
        size: Set(content.len() as i64),
        local_path: Set(local_path.clone()),
        description: Set(None),
        url: Set(None),
        status: Set(crate::models::files::FileUploadStatus::Uploaded),
        created_at: Set(now),
        updated_at: Set(now),
        metadata: Set(None),
    };
    file_row.insert(db).await.map_err(|e| {
        eprintln!("file row insert error: {e}");
        AppError::DbTimeout
    })?;

    Ok((file_uuid, local_path))
}
