use anyhow::{Error,anyhow};
use async_trait::async_trait;
use reqwest::{Client as ReqwestClient, RequestBuilder, multipart};
use reqwest_eventsource::EventSource;
use uuid::Uuid;
use crate::{config::setting::OpenaiSettings, dto::{files::Attachment, llm::openai::{FileUploadResponse, OpenaiChatCompletionRequest, OpenaiChatCompletionResponse, OpenaiChatRequest, OpenaiListModelsResponse, OpenaiMessage, OpenaiModel, OpenaiTool}}, handlers::file::get_file_binary, llm::{prompt::{Prompt, PromptTitleResponse}, provider::{OpenaiApis, OpenaiHeaders}}};

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

   async fn openai_chat_stream(&self,openai_settings:&OpenaiSettings,model_name:String,temperature:Option<f32>,mut prompts:Vec<Prompt>,user_id:&Uuid,web_search:bool) -> Result<EventSource,Error>{
       for prompt in &mut prompts {
         for file in &mut prompt.files {
            if let Ok(attachment) = get_file_binary(&file, user_id){
               file.openai_id = self
                 .openai_upload_file(openai_settings, &attachment)
                 .await
                 .ok()
            }
         }
       }
       let tools = if web_search {
         Some(vec![OpenaiTool::web_search()])
       }else{
         None
       };
       let body = OpenaiChatRequest {
            model: model_name,
            stream: true,
            temperature,
            input:OpenaiMessage::from_prompts(prompts),
            tool_choice:None,
            tools:tools,
            include:None,
        };
      let request = self
            .post(format!("{OPENAI_API_URL}/v1/responses"))
            .add_openai_headers(openai_settings)
            .json(&body);
     let es = EventSource::new(request)?;
     Ok(es)
   }

    async fn openai_chat_stream_text(&self,openai_sesstings:&OpenaiSettings,model_name:String,temperature:Option<f32>,prompt:Vec<String>) -> Result<EventSource,Error>{
       let body = OpenaiChatCompletionRequest {
            model: model_name,
            stream: true,
            temperature,
            messages:vec![OpenaiMessage::from_text(prompt)],
        };
      let request = self
            .post(format!("{OPENAI_API_URL}/v1/chat/completions"))
            .add_openai_headers(openai_sesstings)
            .json(&body);
     let es = EventSource::new(request)?;
     Ok(es)
   }

    async fn openai_get_title(&self,openai_settings:&OpenaiSettings,prompt:String) -> Result<PromptTitleResponse,Error>{
      let title_prompt = format!("Write a short title for the given prompt respond only in title name: {prompt}");
        let body = OpenaiChatCompletionRequest {
            model: "o4-mini".to_string(),
            stream: false,
            temperature:None,
            messages:vec![OpenaiMessage::from_text(vec![title_prompt])],
        };
      let response:OpenaiChatCompletionResponse = self
          .post(format!("{OPENAI_API_URL}/v1/chat/completions"))
          .add_openai_headers(&openai_settings)
          .json(&body)
          .send()
          .await?
          .json()
          .await?;
     let title = response
          .choices
          .first()
          .take()
          .map(|choice| choice.message.content.clone())
          .ok_or(anyhow!("openai response choices is empty"))?
          .ok_or(anyhow!("openai choice message content is empty"))?;
     let (input_tokens,output_tokens) = response
          .usage
          .map(|usage| (usage.prompt_tokens as i32,usage.completion_tokens as i32))
          .unwrap_or((0,0)); 
     Ok(PromptTitleResponse{title,input_tokens,output_tokens})      
    }

    async fn openai_list_models(&self,openai_settings: &OpenaiSettings) -> Result<Vec<OpenaiModel>, Error> {
        let res = self
            .get(format!("{OPENAI_API_URL}/v1/models"))
            .add_openai_headers(openai_settings)
            .send()
            .await?
            .error_for_status()?
            .json::<OpenaiListModelsResponse>()
            .await?;
        Ok(res.data)
    }
}

