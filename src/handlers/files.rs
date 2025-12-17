use std::{fs::{self, File}, io::Write, path::PathBuf};
use axum::{Json, extract::State};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::StatusCode;
use uuid::Uuid;
use crate::{auth::claims::Claims, dto::files::{File as FileUploadResponse, FileUploadRequest}, error::AppError, llm::provider::OpenaiApis, state::SharedState};

pub const LOCAL_FOLDER:&str = "./files";

/// Get the local file path for a given user and file ID
pub fn get_local_file_path(user_id: &Uuid, file_id: &str) -> PathBuf {
    PathBuf::from(format!("{}/{}/{}", LOCAL_FOLDER, user_id, file_id))
}

/// Read a file from local storage and return as base64
pub fn read_file_as_base64(user_id: &Uuid, file_id: &str) -> Option<String> {
    let path = get_local_file_path(user_id, file_id);
    fs::read(&path).ok().map(|bytes| BASE64.encode(&bytes))
}

#[utoipa::path(
    post,
    path = "/files",
    tag = "files",
    request_body = FileUploadRequest,
    responses(
        (status = 200, body = FileUploadResponse),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
    ),
)]
pub async fn upload_file(
   claims:Claims,
   State(app_state):State<SharedState>,
   Json(req):Json<FileUploadRequest>
) -> Result<(StatusCode,Json<FileUploadResponse>),AppError>{
 // Generate a unique local file ID
 let local_file_id = Uuid::new_v4().to_string();
 let user_folder = format!("{}/{}", LOCAL_FOLDER, claims.user_id);

 // Ensure user folder exists
 let _ = fs::create_dir_all(&user_folder);

 // Save file locally with unique ID
 if let Ok(mut file_handle) = File::create(format!("{}/{}", user_folder, &local_file_id)) {
      if let Some(buffer) = &req.attachment.file {
         if file_handle.write_all(buffer).is_ok() {
          println!("Saved file {} locally as {}", &req.attachment.name, &local_file_id)
        }
      }
 }

 let mut response = FileUploadResponse {
     id: Some(local_file_id.clone()),
     size: Some(req.attachment.file.as_ref().map(|f| f.len()).unwrap_or(0)),
     content_type: req.attachment.content_type.clone(),
     name: req.attachment.name.clone(),
 };

 if let Some(provider) = req.provider {
    match provider.to_lowercase().as_str() {
        "openai" => {
            let openai_settings = app_state
                .settings
                .openai
                .as_ref()
                .ok_or(AppError::LlmProviderNotConfigured)?;
            let file_id = app_state
                .req_client
                .openai_upload_file(openai_settings, &req.attachment)
                .await
                .map_err(|e| {
                    eprintln!("Openai file upload error: {}", e);
                    AppError::ServiceTemporarilyUnavailable
                })?;
            // Use OpenAI's file ID for API calls, keep local ID for reference
            response.id = Some(file_id);
        }
        "anthropic" => {
            // Anthropic uses local file ID - data will be loaded from disk during chat
            // Local file ID is already set above
        }
        _ => return Err(AppError::InvalidLlmProvider),
    }
 }
 Ok((StatusCode::OK, Json(response)))
}