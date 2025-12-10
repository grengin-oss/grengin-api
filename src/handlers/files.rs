use std::{fs::File, io::Write};
use axum::{Json, extract::State};
use reqwest::StatusCode;
use crate::{dto::files::{FileUploadRequest, File as FileUploadResponse}, error::AppError, llm::provider::OpenaiApis, state::SharedState};

pub const LOCAL_FOLDER:&str = "./files";

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
   State(app_state):State<SharedState>,
   Json(req):Json<FileUploadRequest>
) -> Result<(StatusCode,Json<FileUploadResponse>),AppError>{
 if let Ok(mut file_path) = File::create(format!("{}/{}",LOCAL_FOLDER,&req.attactment.name)){
      if let Some(buffer) = &req.attactment.file{
         if file_path.write_all(buffer).is_ok(){
          println!("Saved file {} locally",&req.attactment.name)
        }
      }
 }
 let mut response = FileUploadResponse{
     id:None,
     size:Some(req.attactment.file.as_ref().map(|f| f.len()).unwrap_or(0)),
     content_type:req.attactment.content_type.clone(),
     name:req.attactment.name.clone()
 };
 if let Some(provider) = req.provider{ 
 let openai_settings = match provider.to_lowercase().as_str() {
     "openai" => app_state
                   .settings
                   .openai
                   .as_ref()
                   .ok_or(AppError::LlmProviderNotConfigured)?,
       _ => return Err(AppError::InvalidLlmProvider)
   };
  let file_id = app_state
     .req_client
     .openai_upload_file(openai_settings, &req.attactment)
     .await
     .map_err(|e|{
       eprintln!("Openai file upload error:{}",e);
       AppError::ServiceTemporarilyUnavailable})?;
     response.id = Some(file_id);
  }
 Ok((StatusCode::OK,Json(response)))
}