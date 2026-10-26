// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use llm_plugin::{ProviderManifestV1, ProviderModelType};
use std::collections::HashSet;

pub fn starter_models(manifest: &ProviderManifestV1) -> Vec<String> {
    let mut models = manifest
        .models
        .iter()
        .filter(|model| {
            model.model_type != ProviderModelType::TextEmbedder
                && model
                    .metadata
                    .get("deprecated")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
                && !matches!(
                    model
                        .metadata
                        .get("status")
                        .and_then(serde_json::Value::as_str),
                    Some("deprecated" | "retired" | "inactive")
                )
        })
        .collect::<Vec<_>>();
    let date = |value: &serde_json::Value| {
        value
            .get("launchDate")
            .and_then(serde_json::Value::as_str)
            .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
    };
    models.sort_by(|a, b| {
        date(&b.metadata)
            .cmp(&date(&a.metadata))
            .then_with(|| a.id.cmp(&b.id))
    });
    let image = models
        .iter()
        .find(|model| model.model_type == ProviderModelType::ImageGenerator);
    let mut seen = HashSet::new();
    let mut selected = models
        .iter()
        .filter(|model| model.model_type == ProviderModelType::TextGenerator)
        .filter(|model| seen.insert(model.id.clone()))
        .take(if image.is_some() { 2 } else { 3 })
        .map(|model| model.id.clone())
        .collect::<Vec<_>>();
    if let Some(image) = image {
        selected.push(image.id.clone());
    }
    selected
}

pub fn enabled_after_key(previous: bool, first_key: bool, explicit: Option<bool>) -> bool {
    explicit.unwrap_or(previous || first_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_bounded_recent_and_exclude_embeddings_and_retired_models() {
        let mut manifest = ProviderManifestV1::from_json(include_bytes!(
            "../../llm-plugin/examples/openai-compatible.provider.json"
        ))
        .unwrap();
        manifest.models = serde_json::from_value(json!([
            {"id":"old","name":"Old","modelType":"text_generator","metadata":{"launchDate":"2024-01-01"}},
            {"id":"image-old","name":"Old image","modelType":"image_generator","metadata":{"launchDate":"2024-01-01"}},
            {"id":"embed","name":"Embedding","modelType":"text_embedder","metadata":{"launchDate":"2026-10-01"}},
            {"id":"new","name":"New","modelType":"text_generator","metadata":{"launchDate":"2026-10-01"}},
            {"id":"retired","name":"Retired","modelType":"text_generator","metadata":{"launchDate":"2026-10-02","deprecated":true}},
            {"id":"second","name":"Second","modelType":"text_generator","metadata":{"launchDate":"2026-09-01"}},
            {"id":"image","name":"Image","modelType":"image_generator","metadata":{"launchDate":"2026-09-01"}}
        ])).unwrap();
        assert_eq!(starter_models(&manifest), vec!["new", "second", "image"]);
        manifest
            .models
            .retain(|model| model.model_type == ProviderModelType::TextEmbedder);
        assert!(starter_models(&manifest).is_empty());
    }

    #[test]
    fn explicit_disabled_state_wins_and_keyless_engines_stay_disabled() {
        assert!(enabled_after_key(false, true, None));
        assert!(!enabled_after_key(false, true, Some(false)));
        assert!(!enabled_after_key(false, false, None));
        assert!(!enabled_after_key(false, false, Some(false)));
        assert!(enabled_after_key(true, false, None));
    }
}
