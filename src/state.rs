use anyhow::Error;
use sea_orm::{Database, DatabaseConnection};
use tokio::sync::RwLock;
use std::sync::Arc;
use reqwest::Client as ReqwestClient;
use crate::{auth::{azure::build_azure_client, google::build_google_client}, config::setting::{OidcClient, Settings}, dto::oauth::AuthProvider, models::users};

pub struct AppState {
    pub database:DatabaseConnection,
    pub google_client:RwLock<OidcClient>,
    pub azure_client:RwLock<OidcClient>,
    pub req_client:ReqwestClient,
    pub settings:Settings,
}

impl AppState {
    pub async fn from_settings(mut settings:Settings) -> Result<SharedState,Error> {
         let req_client =  reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
         let database = Database::connect(&settings.auth.database_url).await?;
         let _ = settings
           .load_ai_engines_from_db(&database)
           .await
           .map_err(|e|eprintln!("Load ai engines from db error {e}"));
         let google_client = RwLock::new(build_google_client(&req_client,&settings.google.client_id, &settings.google.client_secret, &settings.google.redirect_url).await?);
         let azure_client = RwLock::new(build_azure_client(&req_client,&settings.azure.client_id, &settings.azure.client_secret, &settings.azure.redirect_url,&settings.azure.tenant_id).await?);
         let state =  Self { database, google_client,azure_client,req_client,settings };
        Ok(Arc::new(state))
    }

    pub fn get_oidc_client_and_column_and_redirect_uri(&self, provider: &AuthProvider) -> Result<(&RwLock<OidcClient>, users::Column,&str), Error> {
        match provider.to_lowercase().as_str() {
            "azure" => Ok((&self.azure_client, users::Column::AzureId,&self.settings.azure.redirect_url)),
            "google" => Ok((&self.google_client, users::Column::GoogleId,&self.settings.google.redirect_url)),
            _ => Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        }
    }

    pub async fn refresh_oidc_clinet(&self, provider: &AuthProvider) -> Result<(), Error> {
        match provider.to_lowercase().as_str() {
            "azure" => self.refresh_azure_client().await?,
            "google" => self.refresh_google_client().await?,
            _ => return Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        }
        Ok(())
    } 
 
    async fn refresh_google_client(&self) -> Result<(),Error> {
         let google_client = build_google_client(&self.req_client,&self.settings.google.client_id, &self.settings.google.client_secret, &self.settings.google.redirect_url).await?;
         *self.google_client.write().await = google_client;
         Ok(())
    }

    async fn refresh_azure_client(&self) -> Result<(),Error> {
         let azure_client = build_azure_client(&self.req_client,&self.settings.azure.client_id, &self.settings.azure.client_secret, &self.settings.azure.redirect_url,&self.settings.azure.tenant_id).await?;
         *self.azure_client.write().await = azure_client;
         Ok(())
    }
}

pub type SharedState = Arc<AppState>;