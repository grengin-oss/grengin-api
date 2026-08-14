// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use llm_plugin::ProviderManifestV1;

#[test]
fn reference_openai_compatible_manifest_is_valid() {
    let manifest = ProviderManifestV1::from_json(include_bytes!(
        "../examples/openai-compatible.provider.json"
    ))
    .unwrap();
    assert_eq!(manifest.id, "openai-compatible");
    assert_eq!(manifest.version, "1.0");
    assert!(manifest.capabilities.chat.unwrap().tools);
    assert!(manifest.capabilities.embeddings);
    assert!(manifest.capabilities.image_generation);
}

#[test]
fn complete_example_manifest_is_valid() {
    let manifest = ProviderManifestV1::from_json(include_bytes!("../examples/example.json"))
        .expect("the complete example manifest must remain valid");
    assert_eq!(manifest.version, "1.0");
}

#[test]
fn reference_anthropic_manifest_is_valid() {
    let manifest =
        ProviderManifestV1::from_json(include_bytes!("../examples/anthropic.provider.json"))
            .unwrap();
    assert_eq!(manifest.id, "anthropic");
    assert_eq!(manifest.version, "1.0");
    assert!(manifest.capabilities.chat.unwrap().streaming);
    let model = manifest
        .models
        .iter()
        .find(|model| model.id.as_str() == "claude-haiku-4-5-20251001")
        .expect("reference Anthropic model is missing");
    assert_eq!(model.metadata["inputTokenRate"], 1.0);
    assert_eq!(model.metadata["cachedInputTokenRate"], 0.1);
    assert_eq!(model.metadata["cacheCreationTokenRate"], 1.25);
    assert_eq!(model.metadata["outputTokenRate"], 5.0);
}

#[test]
fn reference_gemini_image_manifest_is_valid() {
    let manifest =
        ProviderManifestV1::from_json(include_bytes!("../examples/gemini-image.provider.json"))
            .unwrap();
    assert_eq!(manifest.id, "gemini-image");
    assert_eq!(manifest.version, "1.0");
    assert!(manifest.capabilities.image_generation);
    assert!(manifest.capabilities.model_listing);
    assert!(manifest.operations.image_generation.is_some());
}

#[test]
fn builtin_chat_manifests_are_valid() {
    for bytes in [
        include_bytes!("../examples/openai.provider.json").as_slice(),
        include_bytes!("../examples/mistral.provider.json").as_slice(),
        include_bytes!("../examples/gemini.provider.json").as_slice(),
    ] {
        let manifest = ProviderManifestV1::from_json(bytes).unwrap();
        assert_eq!(manifest.version, "1.0");
        assert!(manifest.operations.chat_stream.is_some());
        assert!(manifest.capabilities.chat.unwrap().tools);
    }
}

#[test]
fn checked_in_json_schema_is_current() {
    let generated = serde_json::to_value(schemars::schema_for!(ProviderManifestV1)).unwrap();
    let checked_in: serde_json::Value =
        serde_json::from_str(include_str!("../schema/provider-plugin-v1.schema.json")).unwrap();
    assert_eq!(checked_in, generated);
}
