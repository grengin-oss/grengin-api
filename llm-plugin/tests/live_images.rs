// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeMap, env};

use llm_plugin::{
    DeclarativeProvider, ImageRequest, ModelId, ProviderManifestV1, ProviderPlugin,
    ProviderRuntimeConfig,
};
use serde_json::Value;

const OPENAI_COMPATIBLE: &[u8] = include_bytes!("../examples/openai-compatible.provider.json");
const GEMINI_IMAGE: &[u8] = include_bytes!("../examples/gemini-image.provider.json");

fn runtime(api_key: String) -> ProviderRuntimeConfig {
    ProviderRuntimeConfig {
        credentials: BTreeMap::from([("api_key".to_string(), api_key)]),
        default_timeout_ms: 120_000,
        max_response_bytes: 16 * 1024 * 1024,
        ..Default::default()
    }
}

fn credential(name: &str) -> Option<String> {
    if env::var("GRENGIN_LIVE_IMAGE_TESTS").as_deref() != Ok("1") {
        eprintln!("skipping live image test: GRENGIN_LIVE_IMAGE_TESTS is not enabled");
        return None;
    }
    let value = env::var(name).ok().filter(|value| !value.trim().is_empty());
    if value.is_none() {
        eprintln!("skipping live image test: {name} is not configured");
    }
    value
}

fn request(model: String) -> ImageRequest {
    ImageRequest {
        model: ModelId::new(model),
        prompt: "A simple solid red circle centered on a white background. No text.".to_string(),
        input_images: Vec::new(),
        count: 1,
        size: Some("1024x1024".to_string()),
        quality: Some("low".to_string()),
        options: Value::Null,
    }
}

fn assert_image(result: llm_plugin::ImageResult) {
    assert_eq!(result.images.len(), 1);
    let image = &result.images[0];
    assert!(image.bytes.len() > 1_024, "provider returned a tiny image");
    assert!(
        matches!(
            image.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/webp"
        ),
        "unexpected image media type: {}",
        image.media_type
    );
    let known_signature = image.bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || image.bytes.starts_with(b"\xff\xd8\xff")
        || image.bytes.starts_with(b"RIFF");
    assert!(
        known_signature,
        "provider output has no known image signature"
    );
    let usage = result
        .usage
        .expect("provider returned no image token usage");
    assert!(
        usage.input_tokens.is_some_and(|tokens| tokens > 0),
        "provider returned no positive image input-token usage"
    );
    assert!(
        usage.output_tokens.is_some_and(|tokens| tokens > 0),
        "provider returned no positive image output-token usage"
    );
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_IMAGE_TESTS=1 and OPENAI_API_KEY"]
async fn openai_image_generation_smoke() {
    let Some(api_key) = credential("OPENAI_API_KEY") else {
        return;
    };
    let manifest = ProviderManifestV1::from_json(OPENAI_COMPATIBLE).unwrap();
    let provider = DeclarativeProvider::new(manifest, runtime(api_key)).unwrap();
    let model = env::var("OPENAI_IMAGE_MODEL").unwrap_or_else(|_| "gpt-image-1-mini".to_string());
    let result = provider
        .images()
        .unwrap()
        .generate(request(model))
        .await
        .unwrap();
    assert_image(result);
}

#[tokio::test]
#[ignore = "requires GRENGIN_LIVE_IMAGE_TESTS=1 and GEMINI_API_KEY"]
async fn gemini_image_generation_smoke() {
    let Some(api_key) = credential("GEMINI_API_KEY") else {
        return;
    };
    let manifest = ProviderManifestV1::from_json(GEMINI_IMAGE).unwrap();
    let provider = DeclarativeProvider::new(manifest, runtime(api_key)).unwrap();
    let model = env::var("GEMINI_IMAGE_MODEL")
        .unwrap_or_else(|_| "gemini-3.1-flash-lite-image".to_string());
    let result = provider
        .images()
        .unwrap()
        .generate(request(model))
        .await
        .unwrap();
    assert_image(result);
}
