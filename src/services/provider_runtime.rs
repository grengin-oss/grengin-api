// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, sync::Arc};

use llm_plugin::{
    DeclarativeProvider, ProviderError, ProviderId, ProviderManifestV1, ProviderRuntimeConfig,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use thiserror::Error;

use crate::{
    auth::encryption::decrypt_key,
    models::ai_engines::{self, PluginConfig},
    state::AppState,
};

#[derive(Debug, Error)]
pub enum ProviderLoadError {
    #[error("database operation failed")]
    Database,
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error("provider credential could not be decrypted")]
    CredentialDecryption,
}

pub fn parse_plugin_config(value: &serde_json::Value) -> Result<PluginConfig, ProviderLoadError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ProviderError::InvalidManifest(format!("pluginConfig is invalid: {error}")).into()
    })
}

pub fn parse_manifest(config: &PluginConfig) -> Result<ProviderManifestV1, ProviderLoadError> {
    if !config.configuration.is_object() {
        return Err(ProviderError::InvalidManifest(
            "pluginConfig.configuration must be an object".to_string(),
        )
        .into());
    }
    let manifest_bytes = serde_json::to_vec(&config.manifest).map_err(|error| {
        ProviderError::InvalidManifest(format!("manifest serialization failed: {error}"))
    })?;
    let manifest = ProviderManifestV1::from_json(&manifest_bytes)?;
    if manifest.credentials.len() > 1 {
        return Err(ProviderError::InvalidManifest(
            "plugin manifest v1 supports at most one credential in AI engines".to_string(),
        )
        .into());
    }
    Ok(manifest)
}

fn manifest_value(manifest: &ProviderManifestV1) -> Result<serde_json::Value, ProviderLoadError> {
    serde_json::to_value(manifest).map_err(|error| {
        ProviderError::InvalidManifest(format!("manifest serialization failed: {error}")).into()
    })
}

pub fn builtin_plugin_config(engine_key: &str) -> Result<PluginConfig, ProviderLoadError> {
    let bytes = match engine_key {
        "anthropic" => {
            include_bytes!("../../llm-plugin/examples/anthropic.provider.json").as_slice()
        }
        "openai" | "mistral" | "gemini" => {
            include_bytes!("../../llm-plugin/examples/openai-compatible.provider.json").as_slice()
        }
        _ => {
            return Err(ProviderError::InvalidManifest(format!(
                "no embedded manifest exists for AI engine {engine_key}"
            ))
            .into());
        }
    };
    let mut manifest = ProviderManifestV1::from_json(bytes)?;
    let chat_overlay = match engine_key {
        "openai" => {
            Some(include_bytes!("../../llm-plugin/examples/openai.provider.json").as_slice())
        }
        "mistral" => {
            Some(include_bytes!("../../llm-plugin/examples/mistral.provider.json").as_slice())
        }
        "gemini" => {
            Some(include_bytes!("../../llm-plugin/examples/gemini.provider.json").as_slice())
        }
        _ => None,
    };
    if let Some(bytes) = chat_overlay {
        let overlay = ProviderManifestV1::from_json(bytes)?;
        manifest.capabilities.chat = overlay.capabilities.chat;
        manifest.mappings.extend(overlay.mappings);
        manifest.operations.chat_stream = overlay.operations.chat_stream;
    }
    match engine_key {
        "openai" => {
            manifest.id = "openai".to_string();
            manifest.name = "OpenAI".to_string();
        }
        "anthropic" => {}
        "mistral" => {
            manifest.id = "mistral".to_string();
            manifest.name = "Mistral AI".to_string();
            manifest.base_url = "https://api.mistral.ai/v1/".to_string();
            manifest.capabilities.image_generation = false;
            manifest.operations.image_generation = None;
            manifest.operations.image_edit = None;
        }
        "gemini" => {
            manifest.id = "gemini".to_string();
            manifest.name = "Google Gemini".to_string();
            manifest.base_url = "https://generativelanguage.googleapis.com/v1beta/".to_string();
            let embeddings = manifest.operations.embeddings.as_mut().ok_or_else(|| {
                ProviderError::InvalidManifest(
                    "OpenAI-compatible manifest has no embedding operation".to_string(),
                )
            })?;
            embeddings.request.path = "openai/embeddings".to_string();
            let models = manifest.operations.list_models.as_mut().ok_or_else(|| {
                ProviderError::InvalidManifest(
                    "OpenAI-compatible manifest has no model-list operation".to_string(),
                )
            })?;
            models.request.path = "openai/models".to_string();

            let image_manifest = ProviderManifestV1::from_json(include_bytes!(
                "../../llm-plugin/examples/gemini-image.provider.json"
            ))?;
            manifest.capabilities.image_generation = true;
            manifest.mappings.extend(image_manifest.mappings);
            manifest.operations.image_generation = image_manifest.operations.image_generation;
            manifest.operations.image_edit = image_manifest.operations.image_edit;
            manifest.models.extend(image_manifest.models);
        }
        _ => unreachable!(),
    }
    Ok(PluginConfig {
        manifest: manifest_value(&manifest)?,
        configuration: serde_json::json!({}),
        base_url_override: None,
        allow_insecure_http: false,
        allow_private_network: false,
    })
}

pub fn compile_provider(
    config: PluginConfig,
    engine_key: &str,
    api_key: Option<String>,
) -> Result<DeclarativeProvider, ProviderLoadError> {
    let manifest = parse_manifest(&config)?;
    if manifest.id != engine_key {
        return Err(ProviderError::InvalidManifest(
            "manifest id must match the AI engine key".to_string(),
        )
        .into());
    }
    let mut credentials = BTreeMap::new();
    if let (Some(definition), Some(value)) = (manifest.credentials.first(), api_key) {
        credentials.insert(definition.id.clone(), value);
    }
    DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            base_url_override: config.base_url_override,
            configuration: config.configuration,
            credentials,
            allow_insecure_http: config.allow_insecure_http,
            allow_private_network: config.allow_private_network,
            ..Default::default()
        },
    )
    .map_err(ProviderLoadError::from)
}

pub fn build_provider(
    app_key: &[u8; 32],
    engine: &ai_engines::Model,
) -> Result<DeclarativeProvider, ProviderLoadError> {
    let config = match engine.plugin_config.as_ref() {
        Some(config) => parse_plugin_config(config)?,
        None => builtin_plugin_config(&engine.engine_key)?,
    };
    let api_key = engine
        .api_key
        .as_ref()
        .map(|encrypted_value| {
            decrypt_key(app_key, encrypted_value)
                .map_err(|_| ProviderLoadError::CredentialDecryption)
        })
        .transpose()?;
    compile_provider(config, &engine.engine_key, api_key)
}

pub async fn register_provider(
    state: &AppState,
    engine: &ai_engines::Model,
) -> Result<(), ProviderLoadError> {
    let provider = build_provider(&state.settings.auth.app_key, engine)?;
    state.provider_registry.register(Arc::new(provider)).await;
    Ok(())
}

pub async fn unregister_provider(state: &AppState, engine_key: &str) {
    state
        .provider_registry
        .remove(&ProviderId::new(engine_key))
        .await;
}

pub async fn load_enabled_providers(state: &AppState) -> Result<(), ProviderLoadError> {
    let engines = ai_engines::Entity::find()
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .filter(ai_engines::Column::PluginConfig.is_not_null())
        .all(&state.database)
        .await
        .map_err(|_| ProviderLoadError::Database)?;
    for engine in engines {
        if let Err(error) = register_provider(state, &engine).await {
            eprintln!(
                "custom AI engine {} was not loaded: {}",
                engine.engine_key,
                error_class(&error)
            );
        }
    }
    Ok(())
}

fn error_class(error: &ProviderLoadError) -> &'static str {
    match error {
        ProviderLoadError::Database => "database",
        ProviderLoadError::CredentialDecryption => "credential_decryption",
        ProviderLoadError::Provider(_) => "provider_configuration",
    }
}

#[cfg(test)]
mod tests {
    use llm_plugin::{ProviderError, ProviderPlugin};

    use super::*;

    #[test]
    fn all_embedded_providers_compile_through_the_declarative_runtime() {
        for engine_key in ["openai", "anthropic", "mistral", "gemini"] {
            let provider = compile_provider(
                builtin_plugin_config(engine_key).expect("embedded manifest"),
                engine_key,
                Some("test-key".to_string()),
            )
            .expect("compiled provider");
            assert_eq!(provider.descriptor().id.as_str(), engine_key);
        }
    }

    #[test]
    fn manifest_id_must_match_the_ai_engine_key() {
        let error = match compile_provider(
            builtin_plugin_config("openai").expect("embedded manifest"),
            "different-engine",
            Some("test-key".to_string()),
        ) {
            Ok(_) => panic!("mismatched id must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            error,
            ProviderLoadError::Provider(ProviderError::InvalidManifest(_))
        ));
    }

    #[test]
    fn v1_ai_engine_rejects_multiple_credentials() {
        let mut config = builtin_plugin_config("openai").expect("embedded manifest");
        config
            .manifest
            .get_mut("credentials")
            .and_then(serde_json::Value::as_array_mut)
            .expect("credential array")
            .push(serde_json::json!({
                "id": "second_key",
                "type": "secret",
                "required": false
            }));
        let error = parse_manifest(&config).expect_err("multiple credentials must fail");
        assert!(matches!(
            error,
            ProviderLoadError::Provider(ProviderError::InvalidManifest(_))
        ));
    }
}
