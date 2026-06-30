use crate::{
    handlers::projects::{
        add_project_member, add_project_source, create_project, delete_project,
        delete_project_source, get_project, get_project_detail, link_project_to_conversation,
        list_projects, remove_project_member, share_project, unlink_project_from_conversation,
        update_project, update_project_instructions,
    },
    state::SharedState,
};
use axum::{
    Router,
    routing::{delete, get, post, put},
};

pub fn projects_routes() -> Router<SharedState> {
    Router::new()
        .route("/projects", get(list_projects).post(create_project))
        .route(
            "/projects/{id}",
            get(get_project).patch(update_project).delete(delete_project),
        )
        .route("/projects/{id}/detail", get(get_project_detail))
        .route("/projects/{id}/share", post(share_project))
        .route("/projects/{id}/instructions", put(update_project_instructions))
        .route("/projects/{id}/members", post(add_project_member))
        .route("/projects/{id}/members/{user_id}", delete(remove_project_member))
        .route("/projects/{id}/sources", post(add_project_source))
        .route("/projects/{id}/sources/{source_id}", delete(delete_project_source))
        .route(
            "/conversations/{conversation_id}/projects",
            post(link_project_to_conversation),
        )
        .route(
            "/conversations/{conversation_id}/projects/{project_id}",
            delete(unlink_project_from_conversation),
        )
}
