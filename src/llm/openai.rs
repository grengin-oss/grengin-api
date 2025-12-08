use anyhow::{Error, Ok};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder, multipart};
use reqwest_eventsource::EventSource;
use crate::{config::setting::OpenaiSettings, dto::{chat::{Attachment, File}, openai::{FileUploadResponse, OpenaiChatCompletionRequest, OpenaiMessage}}, llm::provider::{OpenaiApis, OpenaiHeaders}};

pub const OPENAI_API_URL:&str = "https://api.openai.com";

impl OpenaiHeaders for RequestBuilder  {
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

#[async_trait]
impl OpenaiApis for ReqwestClient {
    async fn openai_upload_file(&self,openai_settings:&OpenaiSettings,attachment:&Attachment) -> Result<String,Error>{
      let bytes = attachment
        .file
        .as_ref()
        .expect("attachment.file must be Some when uploading");

      let part = multipart::Part::bytes(bytes.clone())
        .file_name(attachment.name.clone())
        .mime_str(&attachment.content_type)?; // e.g. "application/pdf"

      let form = multipart::Form::new()
        .text("purpose", "user_data")
        .part("file", part);

      let res = self.post(format!("{OPENAI_API_URL}/v1/files"))
        .add_openai_headers(openai_settings)
        .multipart(form)
        .send()
        .await?
        .error_for_status()?
        .json::<FileUploadResponse>()
        .await?;
     Ok(res.id)
    }

   async fn openai_chat_stream(&self,openai_sesstings:&OpenaiSettings,model_name:String,prompt:String,temperature:Option<f32>,files:Vec<File>) -> Result<EventSource,Error>{
       let body = OpenaiChatCompletionRequest {
            model: model_name,
            stream: true,
            temperature,
            messages:vec![OpenaiMessage::from_text_and_files(prompt, files)],
        };
      let request = self
            .post(format!("{OPENAI_API_URL}/v1/chat/completions"))
            .add_openai_headers(openai_sesstings)
            .json(&body);
     let es = EventSource::new(request)?;
     Ok(es)
   }
}

