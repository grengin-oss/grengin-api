use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct OpenAIChatCompletionRequest {
    model: String,
    messages: Vec<OpenAIChatCompletionMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

#[derive(Serialize)]
struct OpenAIChatCompletionMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OpenAIChatCompletionResponse {
    choices: Vec<OpenAIChatCompletionChoice>,
}

#[derive(Deserialize)]
struct OpenAIChatCompletionChoice {
    message: OpenAIChatCompletionMessageResponse,
}

#[derive(Deserialize)]
struct OpenAIChatCompletionMessageResponse {
    content: String,
}

#[derive(Deserialize)]
struct OpenAIChatCompletionChunk {
    choices: Vec<OpenAIChatCompletionChunkChoice>,
}

#[derive(Deserialize)]
struct OpenAIChatCompletionChunkChoice {
    delta: OpenAIChatCompletionChunkDelta,
}

#[derive(Deserialize)]
struct OpenAIChatCompletionChunkDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct OpenAIModelList {
    data: Vec<OpenAIModel>,
}

#[derive(Deserialize)]
struct OpenAIModel {
    id: String,
}