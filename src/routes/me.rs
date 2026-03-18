use axum::{Router, routing::{get, post}};
use crate::{
    handlers::me::{
        get_my_administered_department_analytics,
        get_my_administered_department_user_analytics,
        get_my_administered_department_members,
        get_my_administered_departments_list,
        get_my_administered_departments_tree,
        get_my_permissions,
    },
    handlers::notifications::{
        list_my_notifications,
        mark_notification_read,
        stream_my_notifications,
    },
    state::SharedState,
};

pub fn me_routes() -> Router<SharedState> {
    Router::new()
        .route("/me/permissions", get(get_my_permissions))
        .route("/me/notifications", get(list_my_notifications))
        .route("/me/notifications/stream", get(stream_my_notifications))
        .route("/me/notifications/{notification_id}/read", post(mark_notification_read))
        .route("/me/analytics/administered-departments", get(get_my_administered_department_analytics))
        .route(
            "/me/administered-departments",
            get(get_my_administered_departments_list),
        )
        .route(
            "/me/administered-departments/tree",
            get(get_my_administered_departments_tree),
        )
        .route(
            "/me/administered-departments/users",
            get(get_my_administered_department_members),
        )
        .route(
            "/me/analytics/administered-departments/users",
            get(get_my_administered_department_user_analytics),
        )
}
