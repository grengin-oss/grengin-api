use anyhow::Error;
use sea_orm::{Database, DatabaseConnection};
use std::sync::Arc;
use crate::{auth::google::build_google_client, config::setting::{OidcClient, Settings}};

pub struct AppState {
    pub database:DatabaseConnection,
    pub google_client:OidcClient,
    pub settings:Settings,
}

impl AppState {
    pub async fn from_settings(settings:Settings) -> Result<SharedState,Error> {
         let database = Database::connect(&settings.auth.postgres_uri).await?;
         let google_client = build_google_client(&settings.google.client_id, &settings.google.client_secret, &settings.google.redirect_url).await?;
         let state =  Self { database, google_client,settings };
        Ok(Arc::new(state))
    }

    pub async fn get_fresh_google_client(&self) -> Result<OidcClient,Error> {
         let google_client = build_google_client(&self.settings.google.client_id, &self.settings.google.client_secret, &self.settings.google.redirect_url).await?;
       Ok(google_client)
    }
}

pub type SharedState = Arc<AppState>;