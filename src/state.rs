use anyhow::Error;
use sea_orm::{Database, DatabaseConnection};
use tokio::sync::RwLock;
use std::sync::Arc;
use reqwest::Client as ReqwestClient;
use crate::{auth::{azure::build_azure_client, encryption::decrypt_key, google::build_google_client}, config::setting::{ConfigError, OidcClient, Settings}, dto::oauth::AuthProvider, models::users};

pub struct AppState {
    pub database:DatabaseConnection,
    pub google_client:RwLock<Option<OidcClient>>,
    pub azure_client:RwLock<Option<OidcClient>>,
    pub req_client:ReqwestClient,
    pub settings:Settings,
}

impl AppState {
    pub async fn from_settings(mut settings:Settings) -> Result<SharedState,ConfigError> {
         let req_client =  reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| ConfigError::ReqwestClientBuildError(e.to_string()))?;
         let database = Database::connect(&settings.auth.database_url)
           .await
           .map_err(|e| ConfigError::DbError(e.to_string()))?;
         let _ = settings
           .load_ai_engines_from_db(&database)
           .await
           .map_err(|e|eprintln!("Loading ai engines from db error: {e}"));
        let _ = settings
           .load_sso_providers_from_db(&database)
           .await
           .map_err(|e|eprintln!("Loading sso providers from db error: {e}"));
         let state =  Self { 
            database,
            google_client:RwLock::new(None),
            azure_client:RwLock::new(None),
            req_client,settings
         };
         state.refresh_azure_client()
          .await?;
         state.refresh_google_client()
          .await?;
        Ok(Arc::new(state))
    }

    pub async fn check_sso_provider_is_enabled(&self,provider:&AuthProvider) -> Option<bool> {
           match provider.to_lowercase().as_str() {
            "azure" => {
                let is_enabled = self
                   .settings
                   .google
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.is_enabled);
                is_enabled
            },
            "google" => {
                let is_enabled = self
                   .settings
                   .google
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.is_enabled);
                is_enabled
            },
            _ => None
        }
    }

    pub async fn is_email_domain_allowed(&self,email:&str,provider:&AuthProvider) -> bool {
     if let Some((_, domain)) = email.split_once('@') {
         match provider.to_lowercase().as_str() {
            "azure" => {
                let allowed_domains = self
                   .settings
                   .azure
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.allowed_domains.clone())
                   .unwrap_or(Vec::new());
                if allowed_domains.is_empty(){
                    return true;
                }else {
                    return allowed_domains.contains(&domain.to_string());
                }
            },
            "google" => {
                let allowed_domains = self
                   .settings
                   .google
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.allowed_domains.clone())
                   .unwrap_or(Vec::new());
                if allowed_domains.is_empty(){
                    return true;
                }else {
                    return allowed_domains.contains(&domain.to_string());
                }
            },
            _ => return false,
         }
      } 
      false
    }

    pub async fn check_ai_engine_is_enabled(&self,ai_engine_key:&str) -> Option<bool> {
           match ai_engine_key.to_lowercase().as_str() {
            "openai" => {
                let is_enabled = self
                   .settings
                   .openai
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.is_enabled);
                is_enabled
            },
            "anthropic" => {
                let is_enabled = self
                   .settings
                   .anthropic
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.is_enabled);
                is_enabled
            },
            _ => None
        }
    }

    pub async fn get_oidc_client_and_column_and_redirect_uri(&self, provider: &AuthProvider) -> Result<(&RwLock<Option<OidcClient>>, users::Column,Option<String>), ConfigError> {
        match provider.to_lowercase().as_str() {
            "azure" => {
                let redirect_url = self
                   .settings
                   .azure
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.redirect_url.clone());
                return Ok((&self.azure_client, users::Column::AzureId,redirect_url))
            },
            "google" => {
                let redirect_url = self
                   .settings
                   .google
                   .read()
                   .await
                   .as_ref()
                   .map(|setting| setting.redirect_url.clone());
                return Ok((&self.google_client, users::Column::GoogleId,redirect_url))
            },
            _ => Err(ConfigError::InvalidSSoProvider(provider.into())),
        }
    }

    pub async fn refresh_oidc_client(&self, provider: &AuthProvider) -> Result<(), Error> {
        match provider.to_lowercase().as_str() {
            "azure" => self.refresh_azure_client().await?,
            "google" => self.refresh_google_client().await?,
            _ => return Err(anyhow::anyhow!("Unknown provider: {}", provider)),
        }
        Ok(())
    } 
 
    async fn refresh_google_client(&self) -> Result<(),ConfigError> {
          let google = self
             .settings
             .google
             .read()
             .await
             .clone()
             .ok_or(ConfigError::NotConfigured("google settings not configured in App State"))?;
           let google_client = build_google_client(&self.req_client,google.client_id,google.client_secret,google.redirect_url)
             .await
             .map_err(|e| ConfigError::SsoClientBuildError(e.to_string()))?;
           *self.google_client.write().await = Some(google_client);
         Ok(())
    }

    async fn refresh_azure_client(&self) -> Result<(),ConfigError> {
          let azure = self
             .settings
             .azure
             .read()
             .await
             .clone()
             .ok_or(ConfigError::NotConfigured("Azure settings not configured in App State"))?;
           let azure_client = build_azure_client(&self.req_client,azure.client_id,azure.client_secret,azure.redirect_url,azure.tenant_id)
             .await
             .map_err(|e| ConfigError::SsoClientBuildError(e.to_string()))?;
           *self.azure_client.write().await = Some(azure_client);
         Ok(())
    }

    pub fn get_decrypted_api_key_preview(&self,api_key:&Option<String>) -> Option<String> {
      let api_key_preview =  if let Some(api_key_encrypted) = api_key{
         let key = decrypt_key(&self.settings.auth.app_key,api_key_encrypted)
          .ok()
          .unwrap_or(String::new());
          if key.is_empty() {
              Some("<empty>".to_string())
          } else {
          let keep = 4;
          let chars: Vec<char> = key.chars().collect();
          let len = chars.len();
          if len <= keep * 2 {
            Some(key.to_string())
          } else {
            let start: String = chars.iter().take(keep).collect();
            let end: String = chars.iter().skip(len - keep).collect();
           Some(format!("{start}...{end}"))
          }
       }
    }else{
       Some("<empty>".to_string())
    };
 return api_key_preview
}


}

pub type SharedState = Arc<AppState>;