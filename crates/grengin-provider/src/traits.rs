// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use async_trait::async_trait;

use crate::{
    ChatRequest, EmbeddingRequest, EmbeddingResult, ImageRequest, ImageResult, ProviderDescriptor,
    ProviderError, ProviderEventStream, ProviderModel, ToolResult,
};

pub trait ProviderPlugin: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;
    fn chat(&self) -> Option<&dyn ChatProvider>;
    fn embeddings(&self) -> Option<&dyn EmbeddingProvider>;
    fn images(&self) -> Option<&dyn ImageProvider>;
    fn models(&self) -> Option<&dyn ModelProvider>;
}

#[async_trait]
pub trait ChatProvider: Send + Sync {
    async fn start(&self, request: ChatRequest) -> Result<Box<dyn ChatSession>, ProviderError>;
}

#[async_trait]
pub trait ChatSession: Send {
    async fn stream(&mut self) -> Result<ProviderEventStream, ProviderError>;

    async fn continue_with_tools(
        &mut self,
        results: Vec<ToolResult>,
    ) -> Result<ProviderEventStream, ProviderError>;
}

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, request: EmbeddingRequest) -> Result<EmbeddingResult, ProviderError>;
}

#[async_trait]
pub trait ImageProvider: Send + Sync {
    async fn generate(&self, request: ImageRequest) -> Result<ImageResult, ProviderError>;
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn list_models(&self) -> Result<Vec<ProviderModel>, ProviderError>;
}
