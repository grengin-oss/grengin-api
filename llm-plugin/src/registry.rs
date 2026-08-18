// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, sync::Arc};

use tokio::sync::RwLock;

use crate::{ProviderDescriptor, ProviderId, ProviderPlugin};

#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<BTreeMap<ProviderId, Arc<dyn ProviderPlugin>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn register(
        &self,
        provider: Arc<dyn ProviderPlugin>,
    ) -> Option<Arc<dyn ProviderPlugin>> {
        let id = provider.descriptor().id.clone();
        self.providers.write().await.insert(id, provider)
    }

    pub async fn get(&self, id: &ProviderId) -> Option<Arc<dyn ProviderPlugin>> {
        self.providers.read().await.get(id).cloned()
    }

    pub async fn get_by_str(&self, id: &str) -> Option<Arc<dyn ProviderPlugin>> {
        self.get(&ProviderId::new(id)).await
    }

    pub async fn remove(&self, id: &ProviderId) -> Option<Arc<dyn ProviderPlugin>> {
        self.providers.write().await.remove(id)
    }

    pub async fn descriptors(&self) -> Vec<ProviderDescriptor> {
        self.providers
            .read()
            .await
            .values()
            .map(|provider| provider.descriptor().clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        ProviderCapabilities, ProviderDescriptor, ProviderId, ProviderPlugin,
        registry::ProviderRegistry,
    };

    struct TestProvider {
        descriptor: ProviderDescriptor,
    }

    impl TestProvider {
        fn new(id: &str, version: &str) -> Self {
            Self {
                descriptor: ProviderDescriptor {
                    id: ProviderId::new(id),
                    version: version.to_string(),
                    name: id.to_string(),
                    capabilities: ProviderCapabilities::default(),
                },
            }
        }
    }

    impl ProviderPlugin for TestProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn chat(&self) -> Option<&dyn crate::ChatProvider> {
            None
        }

        fn embeddings(&self) -> Option<&dyn crate::EmbeddingProvider> {
            None
        }

        fn images(&self) -> Option<&dyn crate::ImageProvider> {
            None
        }

        fn models(&self) -> Option<&dyn crate::ModelProvider> {
            None
        }
    }

    #[tokio::test]
    async fn replacement_keeps_existing_arc_alive() {
        let registry = ProviderRegistry::new();
        registry
            .register(Arc::new(TestProvider::new("example", "1.0")))
            .await;
        let in_flight = registry.get_by_str("example").await.unwrap();
        let replaced = registry
            .register(Arc::new(TestProvider::new("example", "2.0")))
            .await
            .unwrap();

        assert_eq!(in_flight.descriptor().version, "1.0");
        assert_eq!(replaced.descriptor().version, "1.0");
        assert_eq!(
            registry
                .get_by_str("example")
                .await
                .unwrap()
                .descriptor()
                .version,
            "2.0"
        );
    }
}
