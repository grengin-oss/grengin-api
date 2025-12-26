use axum::{Router, routing::{delete, get, patch, post, put}};
use crate::{handlers::{admin_ai::{delete_ai_engines_api_key_key, get_ai_engine_models_by_key, get_ai_engines, get_ai_engines_by_key, update_ai_engines_by_key, validate_ai_engines_by_key}, admin_department::get_departments, admin_org::{get_org, update_org}, admin_users::{add_new_user, delete_user, get_user_by_id, get_users, patch_user_status, update_user}}, state::SharedState};

pub fn admin_routes() -> Router<SharedState> {
   Router::new()
     .route("/admin/users", get(get_users).post(add_new_user))
     .route("/admin/users/{user_id}",put(update_user).delete(delete_user).get(get_user_by_id))
     .route("/admin/users/{user_id}/status", patch(patch_user_status))
     .route("/admin/organization", get(get_org).put(update_org))
     .route("/admin/ai-engines", get(get_ai_engines))
     .route("/admin/ai-engines/{engine_key}", put(update_ai_engines_by_key).get(get_ai_engines_by_key))
     .route("/admin/departments", get(get_departments))
     .route("/admin/ai-engines/{engine-key}/validate",post(validate_ai_engines_by_key))
     .route("/admin/ai-engines/{engine-key}/api-key",delete(delete_ai_engines_api_key_key))
     .route("/admin/ai-engines/{engine-key}/models",get(get_ai_engine_models_by_key))
}