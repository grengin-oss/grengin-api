// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use grengin_provider::ProviderManifestV1;

#[test]
fn reference_openai_compatible_manifest_is_valid() {
    let manifest = ProviderManifestV1::from_json(include_bytes!(
        "../examples/openai-compatible.provider.json"
    ))
    .unwrap();
    assert_eq!(manifest.id, "openai-compatible");
    assert!(manifest.capabilities.chat.unwrap().tools);
    assert!(manifest.capabilities.embeddings);
    assert!(manifest.capabilities.image_generation);
}

#[test]
fn reference_anthropic_manifest_is_valid() {
    let manifest =
        ProviderManifestV1::from_json(include_bytes!("../examples/anthropic.provider.json"))
            .unwrap();
    assert_eq!(manifest.id, "anthropic");
    assert!(manifest.capabilities.chat.unwrap().streaming);
}

#[test]
fn checked_in_json_schema_is_current() {
    let generated = serde_json::to_value(schemars::schema_for!(ProviderManifestV1)).unwrap();
    let checked_in: serde_json::Value =
        serde_json::from_str(include_str!("../schema/provider-plugin-v1.schema.json")).unwrap();
    assert_eq!(checked_in, generated);
}
