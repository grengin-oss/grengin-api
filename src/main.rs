// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::app::init_app;
use anyhow::Error;

pub mod app;
pub mod auth;
pub mod config;
pub mod database;
pub mod docs;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod routes;
pub mod services;
pub mod state;
pub mod utils;

#[tokio::main]
async fn main() -> Result<(), Error> {
    init_app().await?;
    Ok(())
}
