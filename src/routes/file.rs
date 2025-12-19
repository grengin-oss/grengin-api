use axum::{Router, routing::{get, post}};
use crate::{handlers::file::{download_file, get_file_by_id, get_files, upload_file}, state::SharedState};

pub fn files_routes() -> Router<SharedState> {
   Router::new()
    .route("/files",post(upload_file).get(get_files))
    .route("/files/{file_id}", get(get_file_by_id))
    .route("/files/{file_id}/download", get(download_file))
}