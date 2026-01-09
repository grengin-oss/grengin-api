use crate::{dto::files::File, models::messages::ChatRole};

#[derive(Debug)]
pub struct Prompt {
   pub text:String,
   pub role:ChatRole,
   pub files:Vec<File>,
}

#[derive(Debug)]
pub struct PromptTitleResponse {
   pub title:String,
   pub input_tokens:i32,
   pub output_tokens:i32,
}