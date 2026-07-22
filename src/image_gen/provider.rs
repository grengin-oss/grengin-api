use anyhow::Error;
use async_trait::async_trait;

use crate::config::setting::{GeminiSettings, OpenaiSettings};

pub struct ImageGenResult {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

pub struct InputImage {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

#[async_trait]
pub trait OpenaiImageGenApis {
    async fn openai_generate_image(
        &self,
        settings: &OpenaiSettings,
        model: &str,
        prompt: &str,
        input_images: &[InputImage],
        size: Option<&str>,
        quality: Option<&str>,
    ) -> Result<ImageGenResult, Error>;
}

#[async_trait]
pub trait GeminiImageGenApis {
    async fn gemini_generate_image(
        &self,
        settings: &GeminiSettings,
        model: &str,
        prompt: &str,
        input_images: &[InputImage],
    ) -> Result<ImageGenResult, Error>;
}
