// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::skills::{
        create_skill, delete_skill, get_skill, link_skill, list_conversation_skill_links,
        list_skills, unlink_skill, update_skill,
    },
    state::SharedState,
};
use axum::{
    Router,
    routing::{delete, get, post, put},
};

pub fn skills_routes() -> Router<SharedState> {
    Router::new()
        .route("/skills", get(list_skills))
        .route("/skills/{id}", get(get_skill))
        .route("/admin/skills", post(create_skill))
        .route("/admin/skills/{id}", put(update_skill).delete(delete_skill))
        .route(
            "/conversations/{conversation_id}/skills",
            get(list_conversation_skill_links).post(link_skill),
        )
        .route(
            "/conversations/{conversation_id}/skills/{skill_id}",
            delete(unlink_skill),
        )
}
