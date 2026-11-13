use crate::error::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBrokerIdentity {
    pub distribution_version: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub manifest_sha256: String,
}

impl NativeBrokerIdentity {
    pub fn same_manifest(&self, other: &Self) -> bool {
        self.plugin_id == other.plugin_id
            && self.plugin_version == other.plugin_version
            && self.manifest_sha256 == other.manifest_sha256
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrokerConfiguration {}

pub fn identity(config: &Value) -> Result<Option<NativeBrokerIdentity>, AppError> {
    let Some(broker) = config.get("oauth_broker") else {
        return Ok(None);
    };
    serde_json::from_value::<BrokerConfiguration>(broker.clone())
        .map_err(|_| AppError::ResourceNotFound)?;
    let manifest = config
        .pointer("/connector_plugin/manifest")
        .filter(|manifest| manifest.is_object())
        .ok_or(AppError::ResourceNotFound)?;
    if config.get("oauth").is_some()
        || manifest.get("auth").and_then(Value::as_str) != Some("oauth2")
        || config
            .pointer("/connector_plugin/auth_mode")
            .is_some_and(|mode| mode != "oauth2")
    {
        return Err(AppError::ResourceNotFound);
    }
    let key = manifest
        .get("id")
        .and_then(Value::as_str)
        .ok_or(AppError::ResourceNotFound)?;
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .ok_or(AppError::ResourceNotFound)?;
    if !crate::services::connectors_helpers::valid_catalog_selection(key, version) {
        return Err(AppError::ResourceNotFound);
    }
    Ok(Some(NativeBrokerIdentity {
        distribution_version: env!("CARGO_PKG_VERSION").to_owned(),
        plugin_id: key.to_owned(),
        plugin_version: version.to_owned(),
        manifest_sha256: canonical_digest(manifest)?,
    }))
}

pub fn canonical_digest(value: &Value) -> Result<String, AppError> {
    fn safe_integers(value: &Value) -> bool {
        match value {
            Value::Object(object) => object.values().all(safe_integers),
            Value::Array(values) => values.iter().all(safe_integers),
            Value::Number(number) if number.is_i64() || number.is_u64() => number
                .as_f64()
                .is_some_and(|number| number.abs() <= 9_007_199_254_740_991.0),
            _ => true,
        }
    }
    if !safe_integers(value) {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }
    let bytes = serde_json_canonicalizer::to_vec(value)
        .map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
    Ok(openssl::sha::sha256(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

pub fn same_binding(
    left: Option<&NativeBrokerIdentity>,
    right: Option<&NativeBrokerIdentity>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_manifest(right),
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn broker_is_explicit_and_cannot_mix_direct_oauth_or_api_tokens() {
        assert!(identity(&json!({})).unwrap().is_none());
        let config = json!({"oauth_broker":{},"connector_plugin":{"manifest":{
            "id":"github","version":"1.2.0","auth":"oauth2"}}});
        let valid = identity(&config).unwrap().unwrap();
        assert_eq!(valid.plugin_id, "github");
        for (path, value) in [
            ("oauth", json!({})),
            (
                "oauth_broker",
                json!({"redirect_uri":"https://evil.example"}),
            ),
        ] {
            let mut invalid = config.clone();
            invalid[path] = value;
            assert!(identity(&invalid).is_err());
        }
        let mut invalid = config.clone();
        invalid["connector_plugin"]["auth_mode"] = json!("api_key");
        assert!(identity(&invalid).is_err());
        let mut changed = config;
        changed["connector_plugin"]["manifest"]["base_url"] = json!("https://evil.example");
        assert!(!valid.same_manifest(&identity(&changed).unwrap().unwrap()));
        let mut upgrade = valid.clone();
        upgrade.distribution_version = "0.9.9".into();
        assert!(valid.same_manifest(&upgrade));
    }
    #[test]
    fn numbers_hash_like_javascript_json_stringify() {
        let digest = |raw: &str| canonical_digest(&serde_json::from_str(raw).unwrap());
        assert_eq!(
            digest(r#"{"a":1.0}"#).unwrap(),
            digest(r#"{"a":1}"#).unwrap()
        );
        assert_eq!(
            digest(r#"{"a":-0.0}"#).unwrap(),
            digest(r#"{"a":0}"#).unwrap()
        );
        for (raw, canonical) in [
            (r#"{"a":0.5}"#, r#"{"a":0.5}"#),
            (r#"{"a":1e-7}"#, r#"{"a":1e-7}"#),
            (r#"{"a":1e21}"#, r#"{"a":1e+21}"#),
            (
                r#"{"a":1000000000000000100.0}"#,
                r#"{"a":1000000000000000100}"#,
            ),
        ] {
            let expected = openssl::sha::sha256(canonical.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(digest(raw).unwrap(), expected, "{raw}");
        }
        assert!(digest(r#"{"a":9007199254740993}"#).is_err());
    }

    #[test]
    fn unicode_keys_follow_jcs_utf16_order() {
        let value = json!({"\u{fffd}":1,"\u{1f600}":2});
        let expected = "{\"\u{1f600}\":2,\"\u{fffd}\":1}";
        assert_eq!(
            canonical_digest(&value).unwrap(),
            openssl::sha::sha256(expected.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    }

    #[test]
    fn digest_uses_sorted_json_not_whitespace_or_property_order() {
        let a: Value = serde_json::from_str(r#"{"z":[{"b":2,"a":1}],"a":"content"}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":"content","z":[{"a":1,"b":2}]}"#).unwrap();
        assert_eq!(canonical_digest(&a).unwrap(), canonical_digest(&b).unwrap());
    }
}
