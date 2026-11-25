// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, HashSet},
    sync::LazyLock,
};

use anyhow::{Error, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::dto::models::{ModelInfo, ModelType, PriceTierInfo, ProviderInfo, SpecialtyInfo};

pub const DEFAULT_SPECIALTY: &str = "general";
pub const SPECIALTIES_URL: &str = "https://meta.grengin.com/common/model-specialties.json";
pub const PRICE_TIERS_URL: &str = "https://meta.grengin.com/common/price-tiers.json";

// The same documents the catalog publishes, so tiers work before or without a catalog fetch.
const BUILT_IN_SPECIALTIES: &str = include_str!("../catalog_defaults/model-specialties.json");
const BUILT_IN_PRICE_TIERS: &str = include_str!("../catalog_defaults/price-tiers.json");
const MAX_SUMMARY_CHARS: usize = 200;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpecialtiesDocument {
    format_version: String,
    specialties: Vec<SpecialtyInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PriceTiersDocument {
    format_version: String,
    tiers: Vec<PriceTierInfo>,
    rules: BTreeMap<String, TierRule>,
}

#[derive(Clone, Debug, Deserialize)]
struct TierRule {
    weights: BTreeMap<String, f64>,
    bands: Vec<Band>,
}

#[derive(Clone, Debug, Deserialize)]
struct Band {
    tier: String,
    max: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct PickerMetadata {
    specialties: Vec<SpecialtyInfo>,
    tiers: Vec<PriceTierInfo>,
    rules: BTreeMap<String, TierRule>,
}

static BUILT_IN: LazyLock<PickerMetadata> = LazyLock::new(|| {
    let specialties = parse_specialties(&serde_json::from_str(BUILT_IN_SPECIALTIES).unwrap())
        .expect("built-in model specialties are valid");
    let (tiers, rules) = parse_price_tiers(&serde_json::from_str(BUILT_IN_PRICE_TIERS).unwrap())
        .expect("built-in price tiers are valid");
    PickerMetadata {
        specialties,
        tiers,
        rules,
    }
});

impl PickerMetadata {
    pub fn built_in() -> Self {
        BUILT_IN.clone()
    }

    // A missing or invalid document keeps what this metadata already had (the previous
    // catalog copy on a refresh, the built-in one at start-up).
    pub fn with_catalog(mut self, specialties: Option<Value>, price_tiers: Option<Value>) -> Self {
        if let Some(value) = specialties {
            match parse_specialties(&value) {
                Ok(specialties) => self.specialties = specialties,
                Err(error) => eprintln!("ignoring catalog model specialties: {error}"),
            }
        }
        if let Some(value) = price_tiers {
            match parse_price_tiers(&value) {
                Ok((tiers, rules)) => {
                    self.tiers = tiers;
                    self.rules = rules;
                }
                Err(error) => eprintln!("ignoring catalog price tiers: {error}"),
            }
        }
        self
    }

    pub fn specialties(&self) -> Vec<SpecialtyInfo> {
        self.specialties.clone()
    }

    pub fn price_tiers(&self) -> Vec<PriceTierInfo> {
        self.tiers.clone()
    }

    pub fn annotate(&self, model: &mut ModelInfo) {
        if !self
            .specialties
            .iter()
            .any(|known| known.id == model.specialty)
        {
            model.specialty = DEFAULT_SPECIALTY.to_string();
        }
        model.price_tier = self.price_tier(model);
    }

    // Plugin engines rarely describe themselves; borrow what the catalog knows about the
    // same model (for the Grengin gateway, the upstream model it routes to).
    pub fn enrich_plugin_model(&self, mut model: ModelInfo, catalog: &[ProviderInfo]) -> ModelInfo {
        self.annotate(&mut model);
        let (provider, key) = match model.key.split_once('/') {
            Some((upstream, native)) if model.engine == "grengin" => (upstream, native),
            _ => (model.engine.as_str(), model.key.as_str()),
        };
        let known = catalog
            .iter()
            .find(|candidate| candidate.key.eq_ignore_ascii_case(provider))
            .and_then(|candidate| {
                candidate
                    .models
                    .iter()
                    .find(|entry| entry.key == key || entry.name == key)
            })
            .cloned();
        if let Some(known) = known {
            if model.specialty == DEFAULT_SPECIALTY {
                model.specialty = known.specialty;
            }
            model.recommended |= known.recommended;
            if model.summaries.is_none() {
                model.summaries = known.summaries;
            }
            if model.price_tier.is_none() {
                model.price_tier = known.price_tier;
            }
        }
        model
    }

    fn price_tier(&self, model: &ModelInfo) -> Option<String> {
        let rule = self.rules.get(rule_key(&model.model_type)?)?;
        let mut weighted = 0.0;
        let mut total_weight = 0.0;
        for (field, weight) in rule.weights.iter().filter(|(_, weight)| **weight > 0.0) {
            weighted += weight * rate(model, field)?;
            total_weight += weight;
        }
        let price = weighted / total_weight;
        rule.bands
            .iter()
            .find(|band| band.max.is_none_or(|max| price <= max))
            .map(|band| band.tier.clone())
    }
}

pub fn catalog_specialty(value: &Value) -> String {
    value
        .get("specialty")
        .and_then(Value::as_str)
        .filter(|code| is_code(code))
        .unwrap_or(DEFAULT_SPECIALTY)
        .to_string()
}

pub fn catalog_recommended(value: &Value) -> bool {
    value
        .get("recommended")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

pub fn catalog_summary(value: &Value) -> Option<BTreeMap<String, String>> {
    let entries = value.get("summary")?.as_object()?;
    let summary = entries
        .iter()
        .filter(|(locale, _)| is_locale(locale))
        .filter_map(|(locale, text)| {
            let text = text.as_str()?.trim();
            (!text.is_empty() && text.chars().count() <= MAX_SUMMARY_CHARS)
                .then(|| (locale.to_ascii_lowercase(), text.to_string()))
        })
        .collect::<BTreeMap<_, _>>();
    summary.contains_key("en").then_some(summary)
}

// Resolves each model's summary for one request, on the response copy: the shared cache keeps
// every translation. `fr-CA` tries `fr-ca`, then `fr`, then `en`; a missing or malformed
// locale means English.
pub fn localize(providers: &mut [ProviderInfo], locale: Option<&str>) {
    let chain = locale_chain(locale);
    for model in providers
        .iter_mut()
        .flat_map(|provider| provider.models.iter_mut())
    {
        model.summary = model
            .summaries
            .as_ref()
            .and_then(|summaries| chain.iter().find_map(|tag| summaries.get(tag)))
            .cloned();
    }
}

fn locale_chain(locale: Option<&str>) -> Vec<String> {
    let mut chain = Vec::new();
    let tag = locale
        .map(|tag| tag.trim().replace('_', "-").to_ascii_lowercase())
        .filter(|tag| tag.len() <= 35 && is_locale(tag));
    if let Some(tag) = tag {
        let mut parts = tag.split('-').collect::<Vec<_>>();
        while !parts.is_empty() {
            chain.push(parts.join("-"));
            parts.pop();
        }
    }
    if !chain.iter().any(|tag| tag == "en") {
        chain.push("en".to_string());
    }
    chain
}

fn rule_key(model_type: &ModelType) -> Option<&'static str> {
    match model_type {
        ModelType::TextGenerator => Some("textGenerator"),
        ModelType::ImageGenerator => Some("imageGenerator"),
        ModelType::TextEmbedder => None,
    }
}

fn rate(model: &ModelInfo, field: &str) -> Option<f64> {
    let value = match field {
        "input" => model.input_token_rate,
        "output" => model.output_token_rate,
        "cachedInput" => model.cached_input_token_rate,
        "imageInput" => model.image_input_token_rate,
        // Some image models bill their image output as plain output tokens.
        "imageOutput" => model.image_output_token_rate.or(model.output_token_rate),
        _ => None,
    };
    value.filter(|rate| rate.is_finite() && *rate >= 0.0)
}

const RATE_FIELDS: [&str; 5] = [
    "input",
    "output",
    "cachedInput",
    "imageInput",
    "imageOutput",
];

fn parse_specialties(value: &Value) -> Result<Vec<SpecialtyInfo>, Error> {
    let document: SpecialtiesDocument = serde_json::from_value(value.clone())?;
    ensure_format(&document.format_version)?;
    let mut seen = HashSet::new();
    for specialty in &document.specialties {
        if !is_code(&specialty.id) || !seen.insert(specialty.id.as_str()) {
            bail!("invalid or repeated specialty id {:?}", specialty.id);
        }
        if specialty.label.trim().is_empty() || specialty.description.trim().is_empty() {
            bail!("specialty {} needs a label and a description", specialty.id);
        }
    }
    if !seen.contains(DEFAULT_SPECIALTY) {
        bail!("the {DEFAULT_SPECIALTY} specialty is required");
    }
    Ok(document.specialties)
}

fn parse_price_tiers(
    value: &Value,
) -> Result<(Vec<PriceTierInfo>, BTreeMap<String, TierRule>), Error> {
    let document: PriceTiersDocument = serde_json::from_value(value.clone())?;
    ensure_format(&document.format_version)?;
    if document.tiers.is_empty() {
        bail!("price tiers are empty");
    }
    let mut tiers = HashSet::new();
    for tier in &document.tiers {
        if !is_code(&tier.id) || !tiers.insert(tier.id.as_str()) || tier.label.trim().is_empty() {
            bail!("invalid or repeated price tier {:?}", tier.id);
        }
    }
    for (name, rule) in &document.rules {
        let weights_valid = !rule.weights.is_empty()
            && rule.weights.iter().all(|(field, weight)| {
                RATE_FIELDS.contains(&field.as_str()) && weight.is_finite() && *weight >= 0.0
            })
            && rule.weights.values().sum::<f64>().is_finite()
            && rule.weights.values().sum::<f64>() > 0.0;
        if !weights_valid {
            bail!("price tier rule {name} has invalid weights");
        }
        let mut previous: Option<f64> = None;
        for (index, band) in rule.bands.iter().enumerate() {
            if !tiers.contains(band.tier.as_str()) {
                bail!("price tier rule {name} uses unknown tier {}", band.tier);
            }
            let last = index + 1 == rule.bands.len();
            match band.max {
                None if last => {}
                Some(max)
                    if !last
                        && max.is_finite()
                        && max >= 0.0
                        && previous.is_none_or(|p| max > p) =>
                {
                    previous = Some(max);
                }
                _ => bail!("price tier rule {name} needs ascending limits and an open last band"),
            }
        }
        if rule.bands.is_empty() {
            return Err(anyhow!("price tier rule {name} has no bands"));
        }
    }
    Ok((document.tiers, document.rules))
}

fn ensure_format(version: &str) -> Result<(), Error> {
    if version.split('.').next() == Some("1") {
        Ok(())
    } else {
        bail!("unsupported formatVersion {version}")
    }
}

fn is_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 40
        && value.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn is_locale(value: &str) -> bool {
    let mut parts = value.split('-');
    let language = parts.next().unwrap_or_default();
    (2..=3).contains(&language.len())
        && language.chars().all(|c| c.is_ascii_lowercase())
        && parts.all(|part| {
            (2..=8).contains(&part.len()) && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(model_type: ModelType, input: Option<f64>, output: Option<f64>) -> ModelInfo {
        ModelInfo {
            key: "m".into(),
            name: "M".into(),
            engine: "openai".into(),
            model_type,
            specialty: DEFAULT_SPECIALTY.into(),
            summaries: None,
            summary: None,
            recommended: false,
            price_tier: None,
            comment: None,
            input_token_rate: input,
            output_token_rate: output,
            image_input_token_rate: None,
            image_cached_input_token_rate: None,
            image_output_token_rate: None,
            cached_input_token_rate: None,
            cache_creation_token_rate: None,
            max_input_tokens: None,
            max_output_tokens: None,
            supports_streaming: true,
            supports_tools: true,
            supports_reasoning: false,
            supports_vision: false,
            supports_audio: false,
            supports_pdf_native: false,
            supports_web_search: false,
            supports_multiple_images: false,
            max_images: None,
            dimensions: None,
            price_per_image: None,
        }
    }

    fn tier(
        metadata: &PickerMetadata,
        model_type: ModelType,
        input: f64,
        output: f64,
    ) -> Option<String> {
        metadata.price_tier(&model(model_type, Some(input), Some(output)))
    }

    #[test]
    fn built_in_tiers_blend_three_parts_input_to_one_part_output() {
        let metadata = PickerMetadata::built_in();
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 0.0, 0.0).as_deref(),
            Some("free")
        );
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 0.5, 2.0).as_deref(),
            Some("cheap")
        );
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 1.0, 1.0).as_deref(),
            Some("cheap")
        );
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 2.0, 10.0).as_deref(),
            Some("moderate")
        );
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 5.0, 25.0).as_deref(),
            Some("expensive")
        );
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 15.0, 75.0).as_deref(),
            Some("premium")
        );
        assert_eq!(
            tier(&metadata, ModelType::ImageGenerator, 2.0, 120.0).as_deref(),
            Some("premium")
        );
        assert_eq!(tier(&metadata, ModelType::TextEmbedder, 0.1, 0.0), None);
        assert_eq!(
            metadata.price_tier(&model(ModelType::TextGenerator, Some(1.0), None)),
            None,
            "a model without a known price gets no tier"
        );
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, -5.0, 1.0),
            None,
            "a negative rate is not a price"
        );
        let zero_weight = PickerMetadata::built_in().with_catalog(
            None,
            Some(
                json!({"formatVersion": "1.0", "tiers": [{"id": "low", "label": "Low"}],
                "rules": {"textGenerator": {"weights": {"input": 1, "cachedInput": 0},
                "bands": [{"tier": "low"}]}}}),
            ),
        );
        assert_eq!(
            tier(&zero_weight, ModelType::TextGenerator, 1.0, 1.0).as_deref(),
            Some("low")
        );
        let mut image = model(ModelType::ImageGenerator, Some(5.0), Some(10.0));
        image.image_output_token_rate = Some(8.0);
        assert_eq!(metadata.price_tier(&image).as_deref(), Some("cheap"));
    }

    #[test]
    fn catalog_documents_replace_the_built_in_ones_only_when_valid() {
        let tiers = json!({
            "formatVersion": "1.2",
            "tiers": [{"id": "low", "label": "Low"}, {"id": "high", "label": "High"}],
            "rules": {"textGenerator": {"weights": {"output": 1}, "bands": [
                {"tier": "low", "max": 3}, {"tier": "high"}
            ]}}
        });
        let metadata = PickerMetadata::built_in().with_catalog(None, Some(tiers));
        assert_eq!(metadata.price_tiers().len(), 2);
        assert_eq!(
            tier(&metadata, ModelType::TextGenerator, 100.0, 4.0).as_deref(),
            Some("high")
        );
        assert_eq!(tier(&metadata, ModelType::ImageGenerator, 1.0, 1.0), None);
        assert_eq!(
            metadata.specialties(),
            PickerMetadata::built_in().specialties()
        );

        for broken in [
            json!({"formatVersion": "2.0", "tiers": [], "rules": {}}),
            json!({"formatVersion": "1.0", "tiers": [{"id": "a", "label": "A"}], "rules": {
                "textGenerator": {"weights": {"input": 1}, "bands": [{"tier": "b"}]}}}),
            json!({"formatVersion": "1.0", "tiers": [{"id": "a", "label": "A"}], "rules": {
                "textGenerator": {"weights": {"input": 1}, "bands": [{"tier": "a", "max": 5}, {"tier": "a", "max": 2}, {"tier": "a"}]}}}),
            json!({"formatVersion": "1.0", "tiers": [{"id": "a", "label": "A"}], "rules": {
                "textGenerator": {"weights": {"input": 1}, "bands": [{"tier": "a", "max": 5}]}}}),
            json!({"formatVersion": "1.0", "tiers": [{"id": "a", "label": "A"}], "rules": {
                "textGenerator": {"weights": {"price": 1}, "bands": [{"tier": "a"}]}}}),
            json!({"formatVersion": "1.0", "tiers": [{"id": "A b", "label": "A"}], "rules": {}}),
        ] {
            let metadata = PickerMetadata::built_in().with_catalog(None, Some(broken));
            assert_eq!(
                metadata.price_tiers(),
                PickerMetadata::built_in().price_tiers()
            );
        }
        let previous = metadata.clone();
        let invalid = json!({"formatVersion": "1.0", "tiers": [], "rules": {}});
        assert_eq!(
            previous
                .clone()
                .with_catalog(None, Some(invalid))
                .price_tiers(),
            previous.price_tiers(),
            "a failed or invalid refresh keeps the previous catalog tiers"
        );
        let no_general = json!({"formatVersion": "1.0", "specialties": [
            {"id": "coding", "label": "Coding", "description": "Code"}
        ]});
        assert_eq!(
            PickerMetadata::built_in()
                .with_catalog(Some(no_general), None)
                .specialties(),
            PickerMetadata::built_in().specialties()
        );
    }

    #[test]
    fn catalog_models_keep_known_codes_and_english_led_summaries() {
        let entry = json!({
            "specialty": "coding",
            "summary": {"en": " Strong coder ", "ja": "コードに強い", "EN": "x", "es": "", "fr": 3}
        });
        assert_eq!(catalog_specialty(&entry), "coding");
        assert_eq!(
            catalog_summary(&entry),
            Some(BTreeMap::from([
                ("en".to_string(), "Strong coder".to_string()),
                ("ja".to_string(), "コードに強い".to_string()),
            ]))
        );
        assert_eq!(catalog_summary(&json!({"summary": {"es": "Hola"}})), None);
        assert_eq!(
            catalog_specialty(&json!({"specialty": "Not A Code"})),
            DEFAULT_SPECIALTY
        );
        assert_eq!(catalog_specialty(&json!({})), DEFAULT_SPECIALTY);

        let metadata = PickerMetadata::built_in();
        let mut unknown = model(ModelType::TextGenerator, Some(1.0), Some(1.0));
        unknown.specialty = "time-travel".into();
        metadata.annotate(&mut unknown);
        assert_eq!(unknown.specialty, DEFAULT_SPECIALTY);
        assert_eq!(unknown.price_tier.as_deref(), Some("cheap"));
    }

    #[test]
    fn gateway_models_borrow_catalog_metadata_from_their_upstream_model() {
        let metadata = PickerMetadata::built_in();
        let mut upstream = model(ModelType::TextGenerator, Some(2.0), Some(10.0));
        upstream.key = "gpt-x".into();
        upstream.specialty = "reasoning".into();
        upstream.recommended = catalog_recommended(&json!({"recommended": true}));
        upstream.summaries = Some(BTreeMap::from([("en".into(), "Thinks hard".into())]));
        metadata.annotate(&mut upstream);
        let catalog = vec![ProviderInfo {
            key: "openai".into(),
            name: "OpenAI".into(),
            icon: String::new(),
            icon_dark: String::new(),
            status: "active".into(),
            models: vec![upstream],
        }];
        let mut gateway = model(ModelType::TextGenerator, None, None);
        gateway.engine = "grengin".into();
        gateway.key = "openai/gpt-x".into();
        let enriched = metadata.enrich_plugin_model(gateway, &catalog);
        assert_eq!(enriched.specialty, "reasoning");
        assert_eq!(enriched.price_tier.as_deref(), Some("moderate"));
        assert!(enriched.summaries.is_some());
        assert!(
            enriched.recommended,
            "the gateway model is recommended like its upstream model"
        );
        assert!(!catalog_recommended(&json!({"recommended": "yes"})));
        assert!(!catalog_recommended(&json!({})));

        let mut custom = model(ModelType::TextGenerator, None, None);
        custom.engine = "my-vllm".into();
        let custom = metadata.enrich_plugin_model(custom, &catalog);
        assert_eq!(custom.specialty, DEFAULT_SPECIALTY);
        assert_eq!(custom.price_tier, None);
        assert!(!custom.recommended);
    }

    #[test]
    fn locales_fall_back_from_region_to_language_to_english() {
        assert_eq!(locale_chain(Some("fr-CA")), ["fr-ca", "fr", "en"]);
        assert_eq!(locale_chain(Some(" pt_BR ")), ["pt-br", "pt", "en"]);
        assert_eq!(locale_chain(Some("en-GB")), ["en-gb", "en"]);
        for missing in [None, Some(""), Some("../etc"), Some("x"), Some("fr-")] {
            assert_eq!(locale_chain(missing), ["en"], "{missing:?}");
        }
    }

    #[test]
    fn summaries_resolve_per_request_without_changing_the_cached_catalog() {
        let mut translated = model(ModelType::TextGenerator, None, None);
        translated.summaries = catalog_summary(&json!({"summary": {
            "en": "Writes code", "fr": "Écrit du code", "pt-BR": "Escreve código"
        }}));
        let english_only = model(ModelType::TextGenerator, None, None);
        let cached = vec![ProviderInfo {
            key: "openai".into(),
            name: "OpenAI".into(),
            icon: String::new(),
            icon_dark: String::new(),
            status: "active".into(),
            models: vec![translated, english_only],
        }];
        let resolve = |locale: Option<&str>| {
            let mut response = cached.clone();
            localize(&mut response, locale);
            response[0]
                .models
                .iter()
                .map(|model| model.summary.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            resolve(Some("fr-CA")),
            [Some("Écrit du code".to_string()), None]
        );
        assert_eq!(
            resolve(Some("pt-BR")),
            [Some("Escreve código".to_string()), None]
        );
        assert_eq!(resolve(Some("ja")), [Some("Writes code".to_string()), None]);
        assert_eq!(resolve(None), [Some("Writes code".to_string()), None]);
        assert!(cached[0].models.iter().all(|model| model.summary.is_none()));
        assert_eq!(
            cached[0].models[0].summaries.as_ref().map(BTreeMap::len),
            Some(3)
        );

        let mut response = cached.clone();
        localize(&mut response, Some("fr"));
        let body = serde_json::to_value(&response[0].models[0]).unwrap();
        assert_eq!(body["summary"], "Écrit du code");
        assert!(
            body.get("summaries").is_none(),
            "translations stay internal"
        );
    }
}
