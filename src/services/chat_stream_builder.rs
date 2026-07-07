use crate::config::setting::{AnthropicSettings, GeminiSettings, MistralSettings, OpenaiSettings};

pub enum LlmProviderConfig {
    OpenAI(OpenaiSettings),
    Anthropic(AnthropicSettings),
    Mistral(MistralSettings),
    Gemini(GeminiSettings),
}
