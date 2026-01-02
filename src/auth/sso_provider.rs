use crate::dto::sso_providers::SsoProvider;

pub fn sso_providers_list() -> Vec<SsoProvider>{
   vec![
      SsoProvider{ 
        name:"Google".to_string(),
        provider:"google".to_string(),
        issuer_url:"https://accounts.google.com".to_string(),
        redirect_url:format!("{}/auth/google/callback",std::env::var("REDIRECT_URL").unwrap_or("http://localhost:8080".to_string())), 
     },
      SsoProvider{ 
        name:"Azure".to_string(),
        provider:"azure".to_string(),
        issuer_url: "https://login.microsoftonline.com/<tenant_id>/v2.0".to_string(),
        redirect_url:format!("{}/auth/azure/callback",std::env::var("REDIRECT_URL").unwrap_or("http://localhost:8080".to_string())), 
     }
   ]
}