// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{handlers::branding::get_branding, state::SharedState};
use axum::{Router, routing::get};

pub fn branding_routes() -> Router<SharedState> {
    Router::new().route("/branding", get(get_branding))
}
