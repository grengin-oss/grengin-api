use axum::{Router, routing::{delete, get, patch, post, put}};
use crate::{handlers::{admin_ai::{delete_ai_engines_api_key_key, get_ai_engine_models_by_key, get_ai_engines, get_ai_engines_by_key, update_ai_engines_by_key, validate_ai_engines_by_key}, admin_analytics::{get_analytics_overview, get_department_analytics, get_timeseries_analytics, get_user_analytics}, admin_department::{create_department, delete_department, get_department_by_id, list_departments, update_department}, admin_sso_provider::{delete_sso_provider_by_id, get_sso_provider_by_id, get_sso_providers, update_sso_provider_by_id}, admin_users::{add_new_user, delete_user, get_user_by_id, get_users, patch_user_status, update_user}, branding::{get_admin_branding, update_branding}}, state::SharedState};

pub fn admin_routes() -> Router<SharedState> {
   Router::new()
     .route("/admin/users", get(get_users).post(add_new_user))
     .route("/admin/users/{user_id}",put(update_user).delete(delete_user).get(get_user_by_id))
     .route("/admin/users/{user_id}/status", patch(patch_user_status))
     .route("/admin/branding", get(get_admin_branding).put(update_branding))
     .route("/admin/ai-engines", get(get_ai_engines))
     .route("/admin/ai-engines/{engine_key}", put(update_ai_engines_by_key).get(get_ai_engines_by_key))
     .route("/admin/ai-engines/{engine-key}/validate",post(validate_ai_engines_by_key))
     .route("/admin/ai-engines/{engine-key}/api-key",delete(delete_ai_engines_api_key_key))
     .route("/admin/ai-engines/{engine-key}/models",get(get_ai_engine_models_by_key))
     .route("/admin/sso-providers",get(get_sso_providers))
     .route("/admin/sso-providers/{provider_id}", put(update_sso_provider_by_id).delete(delete_sso_provider_by_id).get(get_sso_provider_by_id))
     .route("/admin/analytics/overview", get(get_analytics_overview))
     .route("/admin/analytics/users", get(get_user_analytics))
     .route("/admin/analytics/departments", get(get_department_analytics))
     .route("/admin/analytics/timeseries", get(get_timeseries_analytics))
     .route("/admin/departments",get(list_departments).post(create_department))
     .route("/admin/department/{department_id}",get(get_department_by_id).put(update_department).delete(delete_department))
}