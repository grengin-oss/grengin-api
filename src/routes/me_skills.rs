// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::me_skills::{
        create_my_skill, delete_my_skill, get_my_skill, list_my_skills, update_my_skill,
    },
    state::SharedState,
};
use axum::{Router, routing::get};

pub fn me_skills_routes() -> Router<SharedState> {
    Router::new()
        .route("/me/skills", get(list_my_skills).post(create_my_skill))
        .route(
            "/me/skills/{id}",
            get(get_my_skill)
                .put(update_my_skill)
                .delete(delete_my_skill),
        )
}
