use serde::{Deserialize, Serialize};
use serde_with::{serde_as, base64::Base64};
use utoipa::{IntoParams, ToSchema};

#[serde_as]
#[derive(Deserialize,Serialize, ToSchema, IntoParams)]
pub struct Attachment {
    #[serde_as(as = "Option<Base64>")]
    #[schema(value_type = String, format = Byte)]
    pub file:Option<Vec<u8>>,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct File {
    pub id:Option<String>,
    pub size: Option<usize>,
    pub name:String,
    #[serde(rename = "type")]
    pub content_type:String
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams)]
pub struct FileUploadRequest{
    pub provider:Option<String>,
    pub attachment:Attachment
}
