use axum::{Router, routing::{get, put}};
use crate::{handlers::admin::{add_new_user, delete_user, get_users, update_user}, state::SharedState};

pub fn admin_routes() -> Router<SharedState> {
   Router::new()
     .route("/admin/user", get(get_users).post(add_new_user))
     .route("/admin/user/{user_id}",put(update_user).delete(delete_user))
}