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

// Mirrors JavaScript's JSON.stringify, which the broker uses for the same digest. Numbers it would
// print differently (exponents, unsafe integers) are refused rather than hashed inconsistently.
fn js_number(number: &serde_json::Number) -> Option<String> {
    const SAFE: f64 = 9_007_199_254_740_991.0;
    if let Some(value) = number.as_i64() {
        return ((value as f64).abs() <= SAFE).then(|| value.to_string());
    }
    if let Some(value) = number.as_u64() {
        return ((value as f64) <= SAFE).then(|| value.to_string());
    }
    let value = number.as_f64()?;
    if value == 0.0 {
        return Some("0".into());
    }
    if !value.is_finite() || !(1e-6..1e21).contains(&value.abs()) {
        return None;
    }
    Some(if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    })
}

pub fn canonical_digest(value: &Value) -> Result<String, AppError> {
    fn write(value: &Value, output: &mut Vec<u8>) -> Result<(), serde_json::Error> {
        match value {
            Value::Object(object) => {
                output.push(b'{');
                let mut keys = object.keys().collect::<Vec<_>>();
                keys.sort();
                for (index, key) in keys.into_iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    serde_json::to_writer(&mut *output, key)?;
                    output.push(b':');
                    write(&object[key], output)?;
                }
                output.push(b'}');
            }
            Value::Array(values) => {
                output.push(b'[');
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    write(value, output)?;
                }
                output.push(b']');
            }
            Value::Number(number) => output.extend_from_slice(
                js_number(number)
                    .ok_or_else(|| {
                        serde::de::Error::custom("number not representable in JSON.stringify")
                    })?
                    .as_bytes(),
            ),
            _ => serde_json::to_writer(output, value)?,
        }
        Ok(())
    }
    let mut bytes = Vec::new();
    write(value, &mut bytes).map_err(|_| AppError::ServiceTemporarilyUnavailable)?;
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
        for (number, javascript) in [
            (0.5, "0.5"),
            (0.000001, "0.000001"),
            (1e20, "100000000000000000000"),
        ] {
            assert_eq!(
                js_number(&serde_json::Number::from_f64(number).unwrap()).unwrap(),
                javascript
            );
        }
        for raw in [
            r#"{"a":1e21}"#,
            r#"{"a":1e-7}"#,
            r#"{"a":9007199254740993}"#,
        ] {
            assert!(digest(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn digest_uses_sorted_json_not_whitespace_or_property_order() {
        let a: Value = serde_json::from_str(r#"{"z":[{"b":2,"a":1}],"a":"content"}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":"content","z":[{"a":1,"b":2}]}"#).unwrap();
        assert_eq!(canonical_digest(&a).unwrap(), canonical_digest(&b).unwrap());
    }
}
