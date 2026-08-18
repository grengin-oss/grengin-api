// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{dto::files::File, models::messages::ChatRole};

#[derive(Debug, Clone)]
pub struct Prompt {
    pub text: String,
    pub role: ChatRole,
    pub files: Vec<File>,
}

#[derive(Debug)]
pub struct PromptTitleResponse {
    pub title: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
}

#[derive(Debug)]
pub struct PromptTextResponse {
    pub text: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
}
