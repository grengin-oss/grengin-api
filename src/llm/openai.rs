use reqwest::RequestBuilder;
use crate::{config::setting::OpenaiSettings, llm::provider::OpenAI};


pub const OPENAI_API_URL:&str = "https://api.openai.com";

impl OpenAI for RequestBuilder  {
    fn add_openai_headers(self,openai_sesstings: &OpenaiSettings) -> Self {
        let mut req_builder = self.bearer_auth(&openai_sesstings.api_key);
        if let Some(openai_project_id) = &openai_sesstings.project_id{
          req_builder = req_builder.header("OpenAI-Organization", openai_project_id);
        }
         if let Some(openai_org_id) = &openai_sesstings.org_id{
          req_builder = req_builder.header("OpenAI-Project", openai_org_id);
        }
        req_builder
    }
}