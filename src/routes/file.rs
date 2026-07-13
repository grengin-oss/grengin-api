use crate::{
    handlers::file::{delete_file_by_id, download_file, get_file_by_id, get_files, upload_file},
    state::SharedState,
};
use axum::{
    Router,
    routing::{get, post},
};

pub fn files_routes() -> Router<SharedState> {
    Router::new()
        .route("/files", post(upload_file).get(get_files))
        .route("/files/{file_id}", get(get_file_by_id).delete(delete_file_by_id))
        .route("/files/{file_id}/download", get(download_file))
}
