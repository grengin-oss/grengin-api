use std::{fs::{self, File}, io::Write, path::PathBuf};
use anyhow::Error;
use axum::{Json, body::Body, extract::{Path, Query, State}, response::{IntoResponse, Response}};
use chrono::Utc;
use migration::extension::postgres::PgExpr;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, Order, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TryIntoModel};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::AuthErrorResponse}, dto::{common::{PaginationQuery, SortRule}, files::{Attachment, File as FileLocal, FilePaginatedResponse, FileResponse, FileUploadRequest}}, error::{AppError, ErrorResponse}, models::files::{self, FileUploadStatus}, state::SharedState};

pub const LOCAL_FOLDER:&str = "/data/files";

pub fn get_local_path(file:&FileLocal,user_id: &Uuid) -> PathBuf {
   let user_folder = format!("{}/{}/file/{}", LOCAL_FOLDER, user_id.to_string(),file.id.to_string());
   let local_path = format!("{}/{}", user_folder,&file.name);
   PathBuf::from(local_path)
}

pub fn get_file_binary(file:&FileLocal,user_id: &Uuid) -> Result<Attachment,Error> {
   let path = get_local_path(file,user_id);
   let buff = fs::read(&path)?;
   Ok(Attachment{
    file:Some(buff),
    name:file.name.clone(),
    content_type:file.content_type.clone(),
  })
}

#[utoipa::path(
    post,
    path = "/files",
    tag = "files",
    request_body = FileUploadRequest,
    responses(
        (status = 200, body = FileResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    ),
)]
pub async fn upload_file(
   claims:Claims,
   State(app_state):State<SharedState>,
   Json(req):Json<FileUploadRequest>
) -> Result<(StatusCode,Json<FileResponse>),AppError>{
 // Generate a unique local file ID
 let local_file_id = Uuid::new_v4();
 let user_folder = format!("{}/{}/file/{}", LOCAL_FOLDER, claims.user_id,local_file_id.to_string());
 // Ensure user folder exists
 let _ = fs::create_dir_all(&user_folder);
 let local_path = format!("{}/{}", user_folder,&req.attachment.name);
 // Save file locally with unique ID
 if let Ok(mut file_handle) = File::create(&local_path) {
      if let Some(buffer) = &req.attachment.file {
         if file_handle.write_all(buffer).is_ok() {
          println!("Saved file {} locally as {}", &req.attachment.name, &user_folder)
        }
      }
 }
 let size = req.attachment
    .file
    .as_ref()
    .map(|f| f.len())
    .unwrap_or(0) as i64;
 let new_file = files::ActiveModel{
    id:Set(local_file_id),
    user_id:Set(claims.user_id),
    name: Set(req.attachment.name.clone()),
    content_type:Set(req.attachment.content_type.clone()),
    size:Set(size),
    local_path:Set(local_path),
    description:Set(req.description),
    url:Set(None),
    status:Set(FileUploadStatus::Uploaded),
    created_at:Set(Utc::now()),
    updated_at:Set(Utc::now()),
    metadata:Set(None), 
 };
 new_file
   .clone()
   .insert(&app_state.database)
   .await
   .map_err(|e|{
      eprintln!("db insert one error: {e}");
      AppError::DbTimeout
    })?;
 let file_model = new_file
   .try_into_model()
   .map_err(|e|{
       eprintln!("file model parse error : {e}");
       AppError::DbTimeout
    })?;  
 let response = FileResponse { 
        id:file_model.id,
        name:file_model.name,
        size:file_model.size,
        content_type:file_model.content_type,
        description:file_model.description,
        url:file_model.url,
        download_url:format!("/files/{}/download",file_model.id.to_string()),
        created_at:file_model.created_at,
        updated_at:file_model.updated_at,
        status:file_model.status 
    };
 Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    get,
    path = "/files/{file_id}/download",
    tag = "files",
    responses(
        (status = 200, description = "file binary with content_type"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "File not found in database (code=5003)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    ),
)]
pub async fn download_file(
   claims:Claims,
   Path(file_id):Path<Uuid>,
   State(app_state):State<SharedState>,
) -> Result<Response<Body>,AppError>{
    let file_model = files::Entity::find_by_id(file_id)
       .filter(files::Column::UserId.eq(claims.user_id))
       .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("db get one error: {e}");
          AppError::DbTimeout
       })?
       .ok_or(AppError::ResourceNotFound)?;
    let file_binary =  fs::read(file_model.local_path)
        .map_err(|e|{
           eprintln!("local storage error : {e}");
           AppError::DbTimeout
       })?;
    let response = Response::builder()
      .status(StatusCode::OK)
      .header("Content-Type",file_model.content_type)
      .body(Body::from(file_binary))
      .map_err(|e|{
          eprintln!("Response builder error: {e}");
          AppError::DbTimeout
       })?
      .into_response();
 Ok(response)
}

#[utoipa::path(
    get,
    path = "/files/{file_id}",
    tag = "files",
    responses(
        (status = 200, body = FileResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "File not found in database (code=5003)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    ),
)]
pub async fn get_file_by_id(
   claims:Claims,
   Path(file_id):Path<Uuid>,
   State(app_state):State<SharedState>,
) -> Result<Json<FileResponse>,AppError>{
    let file_model = files::Entity::find_by_id(file_id)
       .filter(files::Column::UserId.eq(claims.user_id))
       .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("db get one error: {e}");
          AppError::DbTimeout
       })?
       .ok_or(AppError::ResourceNotFound)?;
    let response = FileResponse { 
        id:file_model.id,
        name:file_model.name,
        size:file_model.size,
        content_type:file_model.content_type,
        description:file_model.description,
        url:file_model.url,
        download_url:format!("/files/{}/download",file_model.id.to_string()),
        created_at:file_model.created_at,
        updated_at:file_model.updated_at,
        status:file_model.status 
    };
 Ok(Json(response))
}

#[utoipa::path(
    delete,
    path = "/files/{file_id}",
    tag = "files",
    responses(
        (status = 200, description = "Deleted successfully"),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "File not found in database (code=5003)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    ),
)]
pub async fn delete_file_by_id(
   claims:Claims,
   Path(file_id):Path<Uuid>,
   State(app_state):State<SharedState>,
) -> Result<(StatusCode,&'static str),AppError>{
    let file_model = files::Entity::find_by_id(file_id)
       .filter(files::Column::UserId.eq(claims.user_id))
       .filter(files::Column::Status.eq(FileUploadStatus::Uploaded))
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("db get one error: {e}");
          AppError::DbTimeout
        })?
       .ok_or(AppError::ResourceNotFound)?;
    let mut active_model = file_model
       .into_active_model();
    active_model.status = Set(FileUploadStatus::Deleted);
    active_model.updated_at = Set(Utc::now());
    active_model
       .update(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("db get one error: {e}");
          AppError::DbTimeout
        })?;
 Ok((StatusCode::OK,"Delete successfully"))
}

#[utoipa::path(
    get,
    path = "/files",
    tag = "files",
    
    params(
        ("limit" = Option<u64>, Query, description = "Default value : 20"),
        ("offset" = Option<u64>, Query, description = "Default value : 0"),
        ("search" = Option<String>, Query, description = "Search by file name"),
        ("type" = Option<String>, Query, description = "Search by content_type of file"),
        ("sort" = Option<SortRule>, Query, description = "Sorting by column 'created_at','size','name"),
        ("ascending" = Option<bool>, Query, description = "Sort by ascending order default false"),
    ),
    responses(
        (status = 200, description = "file binary with content_type"),
               (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    ),
)]
pub async fn get_files(
   claims:Claims,
   Query(query):Query<PaginationQuery>,
   State(app_state):State<SharedState>,
) -> Result<Json<FilePaginatedResponse>,AppError>{
    let limit = query.limit.unwrap_or(30);
    let offset = query.offset.unwrap_or(0);
    let page = offset / limit;
    let mut response = FilePaginatedResponse{ 
        files:Vec::new(),
        total:0,
        limit,
        offset
    };
   let mut select = files::Entity::find()
     .offset(offset)
     .limit(limit)
     .filter(files::Column::UserId.eq(claims.user_id));
   if let Some(search) = query.search{
      select = select.filter(files::Column::Name.into_expr().ilike(format!("%{}%", search)));
   }
   if let Some(content_type) = query.content_type{
       select = select.filter(files::Column::ContentType.into_expr().ilike(format!("%{}%", content_type)))
   }
   let order_type = if query.ascending.unwrap_or(false){
     Order::Asc
   }else{
     Order::Desc
   };
   if let Some(sort) = query.sort{
       select = match sort {
          SortRule::Name => select.order_by(files::Column::Name,order_type),
          SortRule::CreatedAt => select.order_by(files::Column::CreatedAt,order_type),
          SortRule::Size => select.order_by(files::Column::Size,order_type),
          _ => select.order_by(files::Column::CreatedAt,order_type)
      };
   }
   let paginator = select
      .paginate(&app_state.database,limit);
   response.total = paginator
      .num_pages()
      .await
      .map_err(|e|{
          eprintln!("db get one error: {e}");
          AppError::DbTimeout
       })?;
   let file_models = paginator.fetch_page(page)
      .await
      .map_err(|e|{
          eprintln!("db get one error: {e}");
          AppError::DbTimeout
       })?;
    response.files = file_models
       .into_iter()
       .map(|file_model|FileResponse{
         id:file_model.id,
         name:file_model.name,
         size:file_model.size,
         content_type:file_model.content_type,
         description:file_model.description,
         url:file_model.url,
         download_url:format!("/files/{}/download",file_model.id.to_string()),
         created_at:file_model.created_at,
         updated_at:file_model.updated_at,
         status:file_model.status 
       })
       .collect::<Vec<_>>();
 Ok(Json(response))
}