// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use anyhow::Error;
use async_trait::async_trait;

use crate::config::setting::{GeminiSettings, OpenaiSettings};

pub struct ImageGenResult {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub text_input_tokens: i32,
    pub image_input_tokens: i32,
    pub output_tokens: i32,
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
        count: u8,
    ) -> Result<Vec<ImageGenResult>, Error>;
}

#[async_trait]
pub trait GeminiImageGenApis {
    async fn gemini_generate_image(
        &self,
        settings: &GeminiSettings,
        model: &str,
        prompt: &str,
        input_images: &[InputImage],
        count: u8,
    ) -> Result<Vec<ImageGenResult>, Error>;
}
