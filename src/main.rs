use anyhow::Error;
use crate::app::init_app;

pub mod app;
pub mod state;
pub mod error;
pub mod dto;
pub mod docs;
pub mod utils;
pub mod models;
pub mod config;
pub mod routes;
pub mod auth;
pub mod handlers;
pub mod database;
pub mod llm;
pub mod services;

#[tokio::main]
async fn main() -> Result<(),Error> {
    init_app().await?;
    Ok(())
}

