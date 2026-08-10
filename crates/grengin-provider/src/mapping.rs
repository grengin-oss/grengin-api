// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ProviderError;

const MAX_MAPPING_DEPTH: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MappingExpression {
    Literal(LiteralExpression),
    Get(GetExpression),
    OmitIfNull(OmitIfNullExpression),
    JsonEncode(JsonEncodeExpression),
    Base64(Base64Expression),
    Map(MapExpression),
    If(IfExpression),
    Switch(SwitchExpression),
    Merge(MergeExpression),
    Object(ObjectExpression),
    Array(ArrayExpression),
    ObjectValue(BTreeMap<String, MappingExpression>),
    ArrayValue(Vec<MappingExpression>),
    Scalar(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LiteralExpression {
    #[serde(rename = "$literal")]
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetExpression {
    #[serde(rename = "$get")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OmitIfNullExpression {
    #[serde(rename = "$omitIfNull")]
    pub value: Box<MappingExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JsonEncodeExpression {
    #[serde(rename = "$jsonEncode")]
    pub value: Box<MappingExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Base64Expression {
    #[serde(rename = "$base64")]
    pub value: Box<MappingExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MapExpression {
    #[serde(rename = "$map")]
    pub path: String,
    pub using: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IfExpression {
    #[serde(rename = "$if")]
    pub condition: Box<MappingExpression>,
    pub then: Box<MappingExpression>,
    #[serde(rename = "else", default)]
    pub else_branch: Option<Box<MappingExpression>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SwitchExpression {
    #[serde(rename = "$switch")]
    pub value: Box<MappingExpression>,
    pub cases: BTreeMap<String, MappingExpression>,
    #[serde(default)]
    pub default: Option<Box<MappingExpression>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MergeExpression {
    #[serde(rename = "$merge")]
    pub values: Vec<MappingExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectExpression {
    #[serde(rename = "$object")]
    pub value: BTreeMap<String, MappingExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArrayExpression {
    #[serde(rename = "$array")]
    pub value: Vec<MappingExpression>,
}

#[derive(Debug, Clone)]
pub struct MappingContext {
    root: Value,
}

impl MappingContext {
    pub fn new(root: Value) -> Self {
        Self { root }
    }

    pub fn for_request<T: Serialize>(
        request: &T,
        configuration: &Value,
        session: &Value,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            root: serde_json::json!({
                "request": serde_json::to_value(request)
                    .map_err(|error| ProviderError::PayloadMapping(error.to_string()))?,
                "config": configuration,
                "session": session,
            }),
        })
    }

    pub fn root(&self) -> &Value {
        &self.root
    }

    fn with_item(&self, item: Value) -> Self {
        let mut root = self.root.clone();
        if let Value::Object(object) = &mut root {
            object.insert("item".to_string(), item);
        }
        Self { root }
    }
}

pub fn validate_mapping_definitions(
    definitions: &BTreeMap<String, MappingExpression>,
) -> Result<(), ProviderError> {
    for (name, expression) in definitions {
        validate_definition_name(name)?;
        let mut stack = vec![name.as_str()];
        validate_expression(expression, definitions, &mut stack, 0)?;
    }
    Ok(())
}

pub fn validate_mapping(
    expression: &MappingExpression,
    definitions: &BTreeMap<String, MappingExpression>,
) -> Result<(), ProviderError> {
    validate_expression(expression, definitions, &mut Vec::new(), 0)
}

fn validate_expression<'a>(
    expression: &'a MappingExpression,
    definitions: &'a BTreeMap<String, MappingExpression>,
    definition_stack: &mut Vec<&'a str>,
    depth: usize,
) -> Result<(), ProviderError> {
    if depth > MAX_MAPPING_DEPTH {
        return Err(ProviderError::InvalidManifest(format!(
            "mapping exceeds maximum nesting depth of {MAX_MAPPING_DEPTH}"
        )));
    }
    let child_depth = depth + 1;
    match expression {
        MappingExpression::Literal(_) | MappingExpression::Scalar(_) => {}
        MappingExpression::Get(get) => validate_canonical_path(&get.path)?,
        MappingExpression::OmitIfNull(value) => {
            validate_expression(&value.value, definitions, definition_stack, child_depth)?
        }
        MappingExpression::JsonEncode(value) => {
            validate_expression(&value.value, definitions, definition_stack, child_depth)?
        }
        MappingExpression::Base64(value) => {
            validate_expression(&value.value, definitions, definition_stack, child_depth)?
        }
        MappingExpression::Map(map) => {
            validate_canonical_path(&map.path)?;
            validate_definition_name(&map.using)?;
            let definition = definitions.get(&map.using).ok_or_else(|| {
                ProviderError::InvalidManifest(format!(
                    "mapping references unknown definition {}",
                    map.using
                ))
            })?;
            if definition_stack.contains(&map.using.as_str()) {
                return Err(ProviderError::InvalidManifest(format!(
                    "mapping definition cycle includes {}",
                    map.using
                )));
            }
            definition_stack.push(&map.using);
            validate_expression(definition, definitions, definition_stack, child_depth)?;
            definition_stack.pop();
        }
        MappingExpression::If(value) => {
            validate_expression(&value.condition, definitions, definition_stack, child_depth)?;
            validate_expression(&value.then, definitions, definition_stack, child_depth)?;
            if let Some(value) = &value.else_branch {
                validate_expression(value, definitions, definition_stack, child_depth)?;
            }
        }
        MappingExpression::Switch(value) => {
            validate_expression(&value.value, definitions, definition_stack, child_depth)?;
            for branch in value.cases.values() {
                validate_expression(branch, definitions, definition_stack, child_depth)?;
            }
            if let Some(branch) = &value.default {
                validate_expression(branch, definitions, definition_stack, child_depth)?;
            }
        }
        MappingExpression::Merge(value) => {
            for child in &value.values {
                validate_expression(child, definitions, definition_stack, child_depth)?;
            }
        }
        MappingExpression::Object(value) => {
            validate_object(&value.value, definitions, definition_stack, child_depth)?;
        }
        MappingExpression::Array(value) => {
            validate_array(&value.value, definitions, definition_stack, child_depth)?;
        }
        MappingExpression::ObjectValue(value) => {
            if let Some(name) = value.keys().find(|name| name.starts_with('$')) {
                return Err(ProviderError::InvalidManifest(format!(
                    "unknown mapping operator {name}"
                )));
            }
            validate_object(value, definitions, definition_stack, child_depth)?;
        }
        MappingExpression::ArrayValue(value) => {
            validate_array(value, definitions, definition_stack, child_depth)?;
        }
    }
    if let MappingExpression::Scalar(value) = expression
        && (value.is_array() || value.is_object())
    {
        return Err(ProviderError::InvalidManifest(
            "compound mapping values must deserialize into typed expressions".to_string(),
        ));
    }
    Ok(())
}

fn validate_object<'a>(
    object: &'a BTreeMap<String, MappingExpression>,
    definitions: &'a BTreeMap<String, MappingExpression>,
    stack: &mut Vec<&'a str>,
    depth: usize,
) -> Result<(), ProviderError> {
    for expression in object.values() {
        validate_expression(expression, definitions, stack, depth)?;
    }
    Ok(())
}

fn validate_array<'a>(
    array: &'a [MappingExpression],
    definitions: &'a BTreeMap<String, MappingExpression>,
    stack: &mut Vec<&'a str>,
    depth: usize,
) -> Result<(), ProviderError> {
    for expression in array {
        validate_expression(expression, definitions, stack, depth)?;
    }
    Ok(())
}

fn validate_definition_name(name: &str) -> Result<(), ProviderError> {
    if name.is_empty()
        || name.len() > 64
        || !name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        return Err(ProviderError::InvalidManifest(format!(
            "mapping definition {name:?} must contain lowercase ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(())
}

fn validate_canonical_path(path: &str) -> Result<(), ProviderError> {
    let root = path
        .strip_prefix('/')
        .and_then(|path| path.split('/').next())
        .or_else(|| path.split('.').next())
        .unwrap_or_default();
    if !matches!(root, "request" | "config" | "session" | "item") {
        return Err(ProviderError::InvalidManifest(format!(
            "mapping path must begin with request, config, session, or item: {path}"
        )));
    }
    Ok(())
}

pub fn evaluate_mapping(
    mapping: &MappingExpression,
    context: &MappingContext,
    definitions: &BTreeMap<String, MappingExpression>,
) -> Result<Value, ProviderError> {
    match evaluate(mapping, context, definitions, false, 0)? {
        Evaluation::Value(value) => Ok(value),
        Evaluation::Omit => Ok(Value::Null),
    }
}

enum Evaluation {
    Value(Value),
    Omit,
}

fn evaluate(
    mapping: &MappingExpression,
    context: &MappingContext,
    definitions: &BTreeMap<String, MappingExpression>,
    optional: bool,
    depth: usize,
) -> Result<Evaluation, ProviderError> {
    if depth > MAX_MAPPING_DEPTH {
        return Err(ProviderError::PayloadMapping(format!(
            "mapping exceeds maximum nesting depth of {MAX_MAPPING_DEPTH}"
        )));
    }
    let child_depth = depth + 1;
    match mapping {
        MappingExpression::Literal(value) => Ok(Evaluation::Value(value.value.clone())),
        MappingExpression::Get(get) => match resolve_path(context.root(), &get.path) {
            Some(value) => Ok(Evaluation::Value(value.clone())),
            None if optional => Ok(Evaluation::Omit),
            None => Err(ProviderError::PayloadMapping(format!(
                "canonical path does not exist: {}",
                get.path
            ))),
        },
        MappingExpression::OmitIfNull(value) => {
            match evaluate(&value.value, context, definitions, true, child_depth)? {
                Evaluation::Value(Value::Null) | Evaluation::Omit => Ok(Evaluation::Omit),
                value => Ok(value),
            }
        }
        MappingExpression::JsonEncode(value) => {
            let value = required_value(evaluate(
                &value.value,
                context,
                definitions,
                false,
                child_depth,
            )?)?;
            Ok(Evaluation::Value(Value::String(
                serde_json::to_string(&value)
                    .map_err(|error| ProviderError::PayloadMapping(error.to_string()))?,
            )))
        }
        MappingExpression::Base64(value) => {
            let value = required_value(evaluate(
                &value.value,
                context,
                definitions,
                false,
                child_depth,
            )?)?;
            let text = value.as_str().ok_or_else(|| {
                ProviderError::PayloadMapping("$base64 requires a string value".to_string())
            })?;
            Ok(Evaluation::Value(Value::String(
                STANDARD.encode(text.as_bytes()),
            )))
        }
        MappingExpression::Map(map) => {
            let Some(selected) = resolve_path(context.root(), &map.path) else {
                if optional {
                    return Ok(Evaluation::Omit);
                }
                return Err(ProviderError::PayloadMapping(format!(
                    "$map path does not exist: {}",
                    map.path
                )));
            };
            let items = selected.as_array().ok_or_else(|| {
                ProviderError::PayloadMapping(format!("$map path is not an array: {}", map.path))
            })?;
            let definition = definitions.get(&map.using).ok_or_else(|| {
                ProviderError::PayloadMapping(format!("unknown mapping definition: {}", map.using))
            })?;
            let mut output = Vec::with_capacity(items.len());
            for item in items {
                let item_context = context.with_item(item.clone());
                if let Evaluation::Value(value) =
                    evaluate(definition, &item_context, definitions, false, child_depth)?
                {
                    output.push(value);
                }
            }
            Ok(Evaluation::Value(Value::Array(output)))
        }
        MappingExpression::If(value) => {
            let condition = required_value(evaluate(
                &value.condition,
                context,
                definitions,
                false,
                child_depth,
            )?)?;
            let condition = condition.as_bool().ok_or_else(|| {
                ProviderError::PayloadMapping("$if condition must evaluate to boolean".to_string())
            })?;
            if condition {
                evaluate(&value.then, context, definitions, false, child_depth)
            } else if let Some(else_branch) = &value.else_branch {
                evaluate(else_branch, context, definitions, false, child_depth)
            } else {
                Ok(Evaluation::Omit)
            }
        }
        MappingExpression::Switch(value) => {
            let selected = required_value(evaluate(
                &value.value,
                context,
                definitions,
                false,
                child_depth,
            )?)?;
            let selected = match selected {
                Value::String(value) => value,
                Value::Bool(value) => value.to_string(),
                Value::Number(value) => value.to_string(),
                _ => {
                    return Err(ProviderError::PayloadMapping(
                        "$switch value must be a string, boolean, or number".to_string(),
                    ));
                }
            };
            if let Some(branch) = value.cases.get(&selected).or(value.default.as_deref()) {
                evaluate(branch, context, definitions, false, child_depth)
            } else {
                Err(ProviderError::PayloadMapping(format!(
                    "$switch has no case for {selected}"
                )))
            }
        }
        MappingExpression::Merge(value) => {
            let mut merged = Map::new();
            for item in &value.values {
                let value =
                    required_value(evaluate(item, context, definitions, false, child_depth)?)?;
                let object = value.as_object().ok_or_else(|| {
                    ProviderError::PayloadMapping("$merge entries must be objects".to_string())
                })?;
                merged.extend(object.clone());
            }
            Ok(Evaluation::Value(Value::Object(merged)))
        }
        MappingExpression::Object(value) => {
            evaluate_object(&value.value, context, definitions, optional, child_depth)
        }
        MappingExpression::ObjectValue(value) => {
            evaluate_object(value, context, definitions, optional, child_depth)
        }
        MappingExpression::Array(value) => {
            evaluate_array(&value.value, context, definitions, child_depth)
        }
        MappingExpression::ArrayValue(value) => {
            evaluate_array(value, context, definitions, child_depth)
        }
        MappingExpression::Scalar(value) => Ok(Evaluation::Value(value.clone())),
    }
}

fn evaluate_object(
    object: &BTreeMap<String, MappingExpression>,
    context: &MappingContext,
    definitions: &BTreeMap<String, MappingExpression>,
    _optional: bool,
    depth: usize,
) -> Result<Evaluation, ProviderError> {
    let mut output = Map::new();
    for (key, child) in object {
        if let Evaluation::Value(value) = evaluate(child, context, definitions, false, depth)? {
            output.insert(key.clone(), value);
        }
    }
    Ok(Evaluation::Value(Value::Object(output)))
}

fn evaluate_array(
    items: &[MappingExpression],
    context: &MappingContext,
    definitions: &BTreeMap<String, MappingExpression>,
    depth: usize,
) -> Result<Evaluation, ProviderError> {
    let mut output = Vec::with_capacity(items.len());
    for item in items {
        if let Evaluation::Value(value) = evaluate(item, context, definitions, false, depth)? {
            output.push(value);
        }
    }
    Ok(Evaluation::Value(Value::Array(output)))
}

fn required_value(evaluation: Evaluation) -> Result<Value, ProviderError> {
    match evaluation {
        Evaluation::Value(value) => Ok(value),
        Evaluation::Omit => Err(ProviderError::PayloadMapping(
            "an omitted value is not valid in this position".to_string(),
        )),
    }
}

pub fn resolve_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }
    if path.starts_with('/') {
        return root.pointer(path);
    }
    let mut current = root;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = match current {
            Value::Object(object) => object.get(segment)?,
            Value::Array(array) => array.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{Value, json};

    use super::{
        MappingContext, MappingExpression, evaluate_mapping, validate_mapping,
        validate_mapping_definitions,
    };

    fn expression(value: Value) -> MappingExpression {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn builds_nested_provider_payload() {
        let context = MappingContext::new(json!({
            "request": {
                "model": "local-model",
                "messages": [
                    { "role": "user", "content": [{"type": "text", "text": "hello"}] }
                ],
                "temperature": null
            },
            "config": {},
            "session": {}
        }));
        let definitions = BTreeMap::from([(
            "message".to_string(),
            expression(json!({
                "speaker": {"$get": "item.role"},
                "parts": {"$get": "item.content"}
            })),
        )]);
        let mapping = expression(json!({
            "engine": {"$get": "request.model"},
            "turns": {"$map": "request.messages", "using": "message"},
            "temperature": {"$omitIfNull": {"$get": "request.temperature"}},
            "stream": {"$literal": true}
        }));
        validate_mapping_definitions(&definitions).unwrap();
        validate_mapping(&mapping, &definitions).unwrap();
        let payload = evaluate_mapping(&mapping, &context, &definitions).unwrap();
        assert_eq!(payload["engine"], "local-model");
        assert_eq!(payload["turns"][0]["speaker"], "user");
        assert_eq!(payload["stream"], true);
        assert!(payload.get("temperature").is_none());
    }

    #[test]
    fn rejects_missing_required_paths_and_unknown_definitions() {
        let context = MappingContext::new(json!({"request": {"messages": []}}));
        assert!(
            evaluate_mapping(
                &expression(json!({"model": {"$get": "request.model"}})),
                &context,
                &BTreeMap::new()
            )
            .is_err()
        );
        let mapping = expression(json!({
            "items": {"$map": "request.messages", "using": "missing"}
        }));
        assert!(validate_mapping(&mapping, &BTreeMap::new()).is_err());
    }

    #[test]
    fn switch_maps_canonical_roles() {
        let context = MappingContext::new(json!({"item": {"role": "assistant"}}));
        let mapping = expression(json!({
            "$switch": {"$get": "item.role"},
            "cases": {
                "user": {"$literal": "human"},
                "assistant": {"$literal": "model"}
            }
        }));
        assert_eq!(
            evaluate_mapping(&mapping, &context, &BTreeMap::new()).unwrap(),
            "model"
        );
    }

    #[test]
    fn rejects_excessive_mapping_depth_and_definition_cycles() {
        let mut mapping = json!("leaf");
        for _ in 0..66 {
            mapping = json!({"nested": mapping});
        }
        assert!(validate_mapping(&expression(mapping), &BTreeMap::new()).is_err());

        let definitions = BTreeMap::from([(
            "recursive".to_string(),
            expression(json!({"$map": "item.children", "using": "recursive"})),
        )]);
        assert!(validate_mapping_definitions(&definitions).is_err());
    }

    #[test]
    fn rejects_unknown_operators_and_undeclared_roots() {
        let unknown = expression(json!({"$execute": "rm -rf"}));
        assert!(validate_mapping(&unknown, &BTreeMap::new()).is_err());
        let environment = expression(json!({"$get": "env.OPENAI_API_KEY"}));
        assert!(validate_mapping(&environment, &BTreeMap::new()).is_err());
    }
}
