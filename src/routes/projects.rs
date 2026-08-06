// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::projects::{
        add_project_artifact, add_project_mcp_server, add_project_member, add_project_source,
        create_project, delete_project, delete_project_artifact, delete_project_source,
        get_project, get_project_artifact, get_project_detail, link_project_to_conversation,
        list_project_artifacts, list_project_mcp_servers, list_project_members, list_projects,
        remove_project_mcp_server, remove_project_member, search_users_for_project, share_project,
        unlink_project_from_conversation, update_project, update_project_artifact,
        update_project_instructions,
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
        .route("/projects/{id}/members/search", get(search_users_for_project))
        .route("/projects/{id}/members", get(list_project_members).post(add_project_member))
        .route("/projects/{id}/members/{user_id}", delete(remove_project_member))
        .route("/projects/{id}/sources", post(add_project_source))
        .route("/projects/{id}/sources/{source_id}", delete(delete_project_source))
        .route("/projects/{id}/artifacts", get(list_project_artifacts).post(add_project_artifact))
        .route(
            "/projects/{id}/artifacts/{artifact_id}",
            get(get_project_artifact)
                .put(update_project_artifact)
                .delete(delete_project_artifact),
        )
        .route(
            "/projects/{id}/mcp-servers",
            get(list_project_mcp_servers).post(add_project_mcp_server),
        )
        .route(
            "/projects/{id}/mcp-servers/{server_id}",
            delete(remove_project_mcp_server),
        )
        .route(
            "/conversations/{conversation_id}/projects",
            post(link_project_to_conversation),
        )
        .route(
            "/conversations/{conversation_id}/projects/{project_id}",
            delete(unlink_project_from_conversation),
        )
}
