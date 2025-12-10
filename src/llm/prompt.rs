use crate::{dto::files::File, models::messages::ChatRole};

#[derive(Debug)]
pub struct Prompt {
   pub text:String,
   pub role:ChatRole,
   pub files:Vec<File>,
}
