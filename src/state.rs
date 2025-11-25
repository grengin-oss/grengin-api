use anyhow::Error;
use sea_orm::{Database, DatabaseConnection};
use tokio::sync::RwLock;
use std::sync::Arc;
use crate::{auth::google::build_google_client, config::setting::{OidcClient, Settings}};

pub struct AppState {
    pub database:DatabaseConnection,
    pub google_client:RwLock<OidcClient>,
    pub settings:Settings,
}

impl AppState {
    pub async fn from_settings(settings:Settings) -> Result<SharedState,Error> {
         let database = Database::connect(&settings.auth.database_url).await?;
         let google_client = RwLock::new(build_google_client(&settings.google.client_id, &settings.google.client_secret, &settings.google.redirect_url).await?);
         let state =  Self { database, google_client,settings };
        Ok(Arc::new(state))
    }

    pub async fn refresh_google_client(&self) -> Result<(),Error> {
         let google_client = build_google_client(&self.settings.google.client_id, &self.settings.google.client_secret, &self.settings.google.redirect_url).await?;
         *self.google_client.write().await = google_client;
         Ok(())
    }
}

pub type SharedState = Arc<AppState>;