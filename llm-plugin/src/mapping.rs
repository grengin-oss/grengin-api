// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use base64::{Engine, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ProviderError;

const MAX_MAPPING_DEPTH: usize = 64;

/// A declarative payload expression.
///
/// Variant order is load-bearing for `untagged` deserialization and must not be rearranged:
///
/// * `ArrayValue` comes first because serde also derives struct-from-sequence for the operator
///   variants below, so a bare JSON array like `[a, b]` would otherwise be read as
///   `IfExpression { condition: a, then: b }`.
/// * The `$`-prefixed operator structs come before `ObjectValue` so `{"$get": …}` is read as an
///   operator instead of a one-key object literal.
/// * `Scalar` comes last because [`Value`] accepts anything.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MappingExpression {
    ArrayValue(Vec<MappingExpression>),
    Literal(LiteralExpression),
    Get(GetExpression),
    OmitIfNull(OmitIfNullExpression),
    JsonEncode(JsonEncodeExpression),
    Base64(Base64Expression),
    Map(MapExpression),
    FlatMap(FlatMapExpression),
    If(IfExpression),
    Switch(SwitchExpression),
    Merge(MergeExpression),
    Concat(ConcatExpression),
    StringConcat(StringConcatExpression),
    Coalesce(CoalesceExpression),
    Object(ObjectExpression),
    Array(ArrayExpression),
    ObjectValue(BTreeMap<String, MappingExpression>),
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
pub struct FlatMapExpression {
    #[serde(rename = "$flatMap")]
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

/// Flattens several arrays into one, the array counterpart of `$merge`.
///
/// Needed whenever a provider takes a single list that the runtime assembles from more than one
/// source, such as `tools` holding both mapped client tools and a literal provider-native tool.
/// Entries that evaluate to nothing are skipped, so an absent optional list contributes nothing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConcatExpression {
    #[serde(rename = "$concat")]
    pub values: Vec<MappingExpression>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StringConcatExpression {
    #[serde(rename = "$stringConcat")]
    pub values: Vec<MappingExpression>,
}

/// Yields the first entry that resolves to a non-null value.
///
/// Needed wherever a provider requires a field the canonical request treats as optional, such as
/// Anthropic's mandatory `max_tokens`, which otherwise fails the whole payload when unset.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CoalesceExpression {
    #[serde(rename = "$coalesce")]
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
        MappingExpression::FlatMap(map) => {
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
        MappingExpression::Concat(value) => {
            for child in &value.values {
                validate_expression(child, definitions, definition_stack, child_depth)?;
            }
        }
        MappingExpression::StringConcat(value) => {
            if value.values.is_empty() {
                return Err(ProviderError::InvalidManifest(
                    "$stringConcat requires at least one entry".to_string(),
                ));
            }
            for child in &value.values {
                validate_expression(child, definitions, definition_stack, child_depth)?;
            }
        }
        MappingExpression::Coalesce(value) => {
            if value.values.is_empty() {
                return Err(ProviderError::InvalidManifest(
                    "$coalesce requires at least one entry".to_string(),
                ));
            }
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
    let (root, _) = split_canonical_path(path);
    if !matches!(root, "request" | "config" | "session" | "item") {
        return Err(ProviderError::InvalidManifest(format!(
            "mapping path must begin with request, config, session, or item: {path}"
        )));
    }
    Ok(())
}

/// Splits a canonical path into its root name and the remainder, keeping the remainder in the
/// same notation as the input so [`resolve_path`] can continue from it.
///
/// `request.messages` -> `("request", "messages")`, `/request/messages` -> `("request", "/messages")`.
fn split_canonical_path(path: &str) -> (&str, &str) {
    if let Some(rest) = path.strip_prefix('/') {
        let end = rest.find('/').unwrap_or(rest.len());
        (&rest[..end], &path[1 + end..])
    } else {
        let end = path.find('.').unwrap_or(path.len());
        (&path[..end], path.get(end + 1..).unwrap_or_default())
    }
}

/// Resolves a canonical path against the request root, or against the current `$map` element when
/// the path is rooted at `item`.
fn resolve_in<'a>(root: &'a Value, item: Option<&'a Value>, path: &str) -> Option<&'a Value> {
    if let Some(item) = item
        && let ("item", rest) = split_canonical_path(path)
    {
        return resolve_path(item, rest);
    }
    resolve_path(root, path)
}

pub fn evaluate_mapping(
    mapping: &MappingExpression,
    context: &MappingContext,
    definitions: &BTreeMap<String, MappingExpression>,
) -> Result<Value, ProviderError> {
    let scope = Scope {
        root: context.root(),
        item: None,
        definitions,
    };
    match evaluate(mapping, scope, false, 0)? {
        Evaluation::Value(value) => Ok(value),
        Evaluation::Omit => Ok(Value::Null),
    }
}

enum Evaluation {
    Value(Value),
    Omit,
}

/// Borrowed evaluation scope. `item` is threaded rather than spliced into `root` so `$map` over a
/// long message list does not deep-clone the whole request per element.
#[derive(Clone, Copy)]
struct Scope<'a> {
    root: &'a Value,
    item: Option<&'a Value>,
    definitions: &'a BTreeMap<String, MappingExpression>,
}

impl<'a> Scope<'a> {
    fn resolve(&self, path: &str) -> Option<&'a Value> {
        resolve_in(self.root, self.item, path)
    }

    fn with_item(&self, item: &'a Value) -> Self {
        Self {
            item: Some(item),
            ..*self
        }
    }
}

fn evaluate(
    mapping: &MappingExpression,
    scope: Scope<'_>,
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
        MappingExpression::Get(get) => match scope.resolve(&get.path) {
            Some(value) => Ok(Evaluation::Value(value.clone())),
            None if optional => Ok(Evaluation::Omit),
            None => Err(ProviderError::PayloadMapping(format!(
                "canonical path does not exist: {}",
                get.path
            ))),
        },
        MappingExpression::OmitIfNull(value) => {
            match evaluate(&value.value, scope, true, child_depth)? {
                Evaluation::Value(Value::Null) | Evaluation::Omit => Ok(Evaluation::Omit),
                value => Ok(value),
            }
        }
        MappingExpression::JsonEncode(value) => {
            let value = required_value(evaluate(&value.value, scope, false, child_depth)?)?;
            Ok(Evaluation::Value(Value::String(
                serde_json::to_string(&value)
                    .map_err(|error| ProviderError::PayloadMapping(error.to_string()))?,
            )))
        }
        MappingExpression::Base64(value) => {
            let value = required_value(evaluate(&value.value, scope, false, child_depth)?)?;
            let text = value.as_str().ok_or_else(|| {
                ProviderError::PayloadMapping("$base64 requires a string value".to_string())
            })?;
            Ok(Evaluation::Value(Value::String(
                STANDARD.encode(text.as_bytes()),
            )))
        }
        MappingExpression::Map(map) => {
            let Some(selected) = scope.resolve(&map.path) else {
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
            let definition = scope.definitions.get(&map.using).ok_or_else(|| {
                ProviderError::PayloadMapping(format!("unknown mapping definition: {}", map.using))
            })?;
            let mut output = Vec::with_capacity(items.len());
            for item in items {
                if let Evaluation::Value(value) =
                    evaluate(definition, scope.with_item(item), false, child_depth)?
                {
                    output.push(value);
                }
            }
            Ok(Evaluation::Value(Value::Array(output)))
        }
        MappingExpression::FlatMap(map) => {
            let Some(selected) = scope.resolve(&map.path) else {
                if optional {
                    return Ok(Evaluation::Omit);
                }
                return Err(ProviderError::PayloadMapping(format!(
                    "$flatMap path does not exist: {}",
                    map.path
                )));
            };
            let items = selected.as_array().ok_or_else(|| {
                ProviderError::PayloadMapping(format!(
                    "$flatMap path is not an array: {}",
                    map.path
                ))
            })?;
            let definition = scope.definitions.get(&map.using).ok_or_else(|| {
                ProviderError::PayloadMapping(format!("unknown mapping definition: {}", map.using))
            })?;
            let mut output = Vec::new();
            for item in items {
                let value = required_value(evaluate(
                    definition,
                    scope.with_item(item),
                    false,
                    child_depth,
                )?)?;
                let values = value.as_array().ok_or_else(|| {
                    ProviderError::PayloadMapping(format!(
                        "$flatMap definition {} must produce an array",
                        map.using
                    ))
                })?;
                output.extend(values.iter().cloned());
            }
            Ok(Evaluation::Value(Value::Array(output)))
        }
        MappingExpression::If(value) => {
            let condition = match evaluate(&value.condition, scope, optional, child_depth)? {
                Evaluation::Omit => return Ok(Evaluation::Omit),
                Evaluation::Value(value) => value,
            };
            let condition = condition.as_bool().ok_or_else(|| {
                ProviderError::PayloadMapping("$if condition must evaluate to boolean".to_string())
            })?;
            if condition {
                evaluate(&value.then, scope, false, child_depth)
            } else if let Some(else_branch) = &value.else_branch {
                evaluate(else_branch, scope, false, child_depth)
            } else {
                Ok(Evaluation::Omit)
            }
        }
        MappingExpression::Switch(value) => {
            let selected = match evaluate(&value.value, scope, optional, child_depth)? {
                Evaluation::Omit => return Ok(Evaluation::Omit),
                Evaluation::Value(value) => value,
            };
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
                evaluate(branch, scope, false, child_depth)
            } else {
                // `$switch` is meant for enumerated fields (roles, content-part types), but a
                // manifest can point it at free text, so bound what lands in the error and logs and
                // name the declared cases to keep the failure diagnosable.
                Err(ProviderError::PayloadMapping(format!(
                    "$switch has no case for {:?} and declares no default; cases are: {}",
                    truncate(&selected, 48),
                    value
                        .cases
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            }
        }
        MappingExpression::Merge(value) => {
            let mut merged = Map::new();
            for item in &value.values {
                let value = required_value(evaluate(item, scope, false, child_depth)?)?;
                let object = value.as_object().ok_or_else(|| {
                    ProviderError::PayloadMapping("$merge entries must be objects".to_string())
                })?;
                merged.extend(object.clone());
            }
            Ok(Evaluation::Value(Value::Object(merged)))
        }
        MappingExpression::Concat(value) => {
            let mut concatenated = Vec::new();
            for item in &value.values {
                // Each entry is optional so a `$map` over an absent list contributes nothing
                // instead of failing the whole payload.
                match evaluate(item, scope, true, child_depth)? {
                    Evaluation::Omit => continue,
                    Evaluation::Value(Value::Null) => continue,
                    Evaluation::Value(Value::Array(values)) => concatenated.extend(values),
                    Evaluation::Value(_) => {
                        return Err(ProviderError::PayloadMapping(
                            "$concat entries must be arrays".to_string(),
                        ));
                    }
                }
            }
            if concatenated.is_empty() && optional {
                return Ok(Evaluation::Omit);
            }
            Ok(Evaluation::Value(Value::Array(concatenated)))
        }
        MappingExpression::StringConcat(value) => {
            let mut output = String::new();
            for item in &value.values {
                let value = required_value(evaluate(item, scope, false, child_depth)?)?;
                match value {
                    Value::String(value) => output.push_str(&value),
                    Value::Bool(value) => output.push_str(if value { "true" } else { "false" }),
                    Value::Number(value) => output.push_str(&value.to_string()),
                    _ => {
                        return Err(ProviderError::PayloadMapping(
                            "$stringConcat entries must be strings, numbers, or booleans"
                                .to_string(),
                        ));
                    }
                }
            }
            Ok(Evaluation::Value(Value::String(output)))
        }
        MappingExpression::Coalesce(value) => {
            for item in &value.values {
                match evaluate(item, scope, true, child_depth)? {
                    Evaluation::Omit | Evaluation::Value(Value::Null) => continue,
                    found => return Ok(found),
                }
            }
            if optional {
                return Ok(Evaluation::Omit);
            }
            Err(ProviderError::PayloadMapping(
                "$coalesce produced no value".to_string(),
            ))
        }
        MappingExpression::Object(value) => evaluate_object(&value.value, scope, child_depth),
        MappingExpression::ObjectValue(value) => evaluate_object(value, scope, child_depth),
        MappingExpression::Array(value) => evaluate_array(&value.value, scope, child_depth),
        MappingExpression::ArrayValue(value) => evaluate_array(value, scope, child_depth),
        MappingExpression::Scalar(value) => Ok(Evaluation::Value(value.clone())),
    }
}

fn evaluate_object(
    object: &BTreeMap<String, MappingExpression>,
    scope: Scope<'_>,
    depth: usize,
) -> Result<Evaluation, ProviderError> {
    let mut output = Map::new();
    for (key, child) in object {
        if let Evaluation::Value(value) = evaluate(child, scope, false, depth)? {
            output.insert(key.clone(), value);
        }
    }
    Ok(Evaluation::Value(Value::Object(output)))
}

fn evaluate_array(
    items: &[MappingExpression],
    scope: Scope<'_>,
    depth: usize,
) -> Result<Evaluation, ProviderError> {
    let mut output = Vec::with_capacity(items.len());
    for item in items {
        if let Evaluation::Value(value) = evaluate(item, scope, false, depth)? {
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

/// Shortens a value for inclusion in an error message, respecting char boundaries.
fn truncate(value: &str, limit: usize) -> String {
    match value.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &value[..end]),
        None => value.to_string(),
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
    fn nested_maps_scope_item_to_the_innermost_element() {
        let context = MappingContext::new(json!({
            "request": {"messages": [
                {"role": "user", "content": [{"text": "a"}, {"text": "b"}]},
                {"role": "assistant", "content": [{"text": "c"}]}
            ]}
        }));
        let definitions = BTreeMap::from([
            (
                "message".to_string(),
                expression(json!({
                    "who": {"$get": "item.role"},
                    "parts": {"$map": "item.content", "using": "part"}
                })),
            ),
            (
                "part".to_string(),
                // Pointer notation must resolve against the element too, and an inner `$map` must
                // not be able to see the outer element.
                expression(json!({"body": {"$get": "/item/text"}})),
            ),
        ]);
        let mapping =
            expression(json!({"turns": {"$map": "request.messages", "using": "message"}}));
        validate_mapping_definitions(&definitions).unwrap();
        validate_mapping(&mapping, &definitions).unwrap();
        let payload = evaluate_mapping(&mapping, &context, &definitions).unwrap();
        assert_eq!(
            payload["turns"],
            json!([
                {"who": "user", "parts": [{"body": "a"}, {"body": "b"}]},
                {"who": "assistant", "parts": [{"body": "c"}]}
            ])
        );
    }

    #[test]
    fn reads_bare_arrays_of_every_length_as_array_literals() {
        // serde derives struct-from-sequence for the operator variants, so an array of 1, 2 or 3
        // elements used to be swallowed by `$literal`, `$map` and `$if` respectively.
        let context = MappingContext::new(json!({"request": {"messages": ["m"]}}));
        for length in 0..5 {
            let items = (0..length)
                .map(|index| json!({"$literal": index}))
                .collect::<Vec<_>>();
            let expected = (0..length).collect::<Vec<_>>();
            let mapping = expression(json!({"stop": items}));
            validate_mapping(&mapping, &BTreeMap::new()).unwrap();
            assert_eq!(
                evaluate_mapping(&mapping, &context, &BTreeMap::new()).unwrap()["stop"],
                json!(expected),
                "array of {length} element(s)"
            );
        }
        // Two bare strings are a two-element array, not `{"$map": …, "using": …}`.
        let mapping = expression(json!({"stop": ["request.messages", "message"]}));
        assert_eq!(
            evaluate_mapping(&mapping, &context, &BTreeMap::new()).unwrap()["stop"],
            json!(["request.messages", "message"])
        );
        // The explicit `$array` form keeps working.
        assert_eq!(
            evaluate_mapping(
                &expression(json!({"$array": [{"$get": "request.messages"}]})),
                &context,
                &BTreeMap::new()
            )
            .unwrap(),
            json!([["m"]])
        );
    }

    #[test]
    fn omit_if_null_drops_object_keys_but_not_array_elements() {
        let context = MappingContext::new(json!({"request": {"present": 1}}));
        let mapping = expression(json!({
            "kept": {"$omitIfNull": {"$get": "request.present"}},
            "dropped": {"$omitIfNull": {"$get": "request.absent"}},
            "list": [{"$omitIfNull": {"$get": "request.absent"}}, {"$literal": 2}]
        }));
        let payload = evaluate_mapping(&mapping, &context, &BTreeMap::new()).unwrap();
        assert_eq!(payload["kept"], 1);
        assert!(payload.get("dropped").is_none());
        assert_eq!(payload["list"], json!([2]));
    }

    #[test]
    fn maps_missing_optional_collections_to_nothing_instead_of_failing() {
        let context = MappingContext::new(json!({"request": {}}));
        let definitions = BTreeMap::from([(
            "tool".to_string(),
            expression(json!({"n": {"$get": "item"}})),
        )]);
        // `$map` over an absent path is only tolerated inside `$omitIfNull`; on its own it is an
        // error so a typo cannot silently produce an empty tool list.
        assert_eq!(
            evaluate_mapping(
                &expression(
                    json!({"tools": {"$omitIfNull": {"$map": "request.tools", "using": "tool"}}})
                ),
                &context,
                &definitions
            )
            .unwrap(),
            json!({})
        );
        assert!(
            evaluate_mapping(
                &expression(json!({"tools": {"$map": "request.tools", "using": "tool"}})),
                &context,
                &definitions
            )
            .is_err()
        );
    }

    #[test]
    fn omit_if_null_propagates_through_conditional_operators() {
        let context = MappingContext::new(json!({"request": {}}));
        let mapping = expression(json!({
            "choice": {
                "$omitIfNull": {
                    "$switch": {"$get": "request.choice.type"},
                    "cases": {"auto": {"$literal": "auto"}}
                }
            },
            "feature": {
                "$omitIfNull": {
                    "$if": {"$get": "request.feature.enabled"},
                    "then": {"$literal": true}
                }
            }
        }));
        assert_eq!(
            evaluate_mapping(&mapping, &context, &BTreeMap::new()).unwrap(),
            json!({})
        );
    }

    #[test]
    fn applies_the_remaining_operators_and_rejects_wrong_types() {
        let context = MappingContext::new(json!({
            "request": {"flag": true, "text": "hi", "payload": {"a": 1}, "number": 4}
        }));
        let payload = evaluate_mapping(
            &expression(json!({
                "encoded": {"$jsonEncode": {"$get": "request.payload"}},
                "based": {"$base64": {"$get": "request.text"}},
                "chosen": {"$if": {"$get": "request.flag"}, "then": {"$literal": "yes"}, "else": {"$literal": "no"}},
                "fallback": {"$switch": {"$get": "request.number"}, "cases": {}, "default": {"$literal": "other"}},
                "merged": {"$merge": [{"$get": "request.payload"}, {"$object": {"b": {"$literal": 2}}}]},
                "joined": {"$stringConcat": [
                    {"$literal": "data:"},
                    {"$get": "request.text"},
                    {"$literal": ";count="},
                    {"$get": "request.number"},
                    {"$literal": ";enabled="},
                    {"$get": "request.flag"}
                ]}
            })),
            &context,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(payload["encoded"], r#"{"a":1}"#);
        assert_eq!(payload["based"], "aGk=");
        assert_eq!(payload["chosen"], "yes");
        assert_eq!(payload["fallback"], "other");
        assert_eq!(payload["merged"], json!({"a": 1, "b": 2}));
        assert_eq!(payload["joined"], "data:hi;count=4;enabled=true");

        for invalid in [
            json!({"$base64": {"$get": "request.payload"}}),
            json!({"$if": {"$get": "request.text"}, "then": {"$literal": 1}}),
            json!({"$merge": [{"$get": "request.text"}]}),
            json!({"$switch": {"$get": "request.payload"}, "cases": {}}),
            json!({"$switch": {"$get": "request.text"}, "cases": {}}),
            json!({"$stringConcat": [{"$get": "request.payload"}]}),
        ] {
            assert!(
                evaluate_mapping(&expression(invalid.clone()), &context, &BTreeMap::new()).is_err(),
                "{invalid} should not evaluate"
            );
        }
    }

    #[test]
    fn concat_flattens_client_and_native_tool_lists() {
        let definitions = BTreeMap::from([(
            "tool".to_string(),
            expression(json!({"name": {"$get": "item.name"}})),
        )]);
        let mapping = expression(json!({
            "tools": {"$omitIfNull": {"$concat": [
                {"$map": "request.tools", "using": "tool"},
                {"$omitIfNull": {"$get": "request.options.nativeTools"}}
            ]}}
        }));
        validate_mapping(&mapping, &definitions).unwrap();

        // Both sources present: one flat array, in order.
        let context = MappingContext::new(json!({
            "request": {
                "tools": [{"name": "mcp__a__x__1"}],
                "options": {"nativeTools": [{"type": "web_search_20250305"}]}
            }
        }));
        assert_eq!(
            evaluate_mapping(&mapping, &context, &definitions).unwrap()["tools"],
            json!([{"name": "mcp__a__x__1"}, {"type": "web_search_20250305"}])
        );

        // Only native tools: the absent client list contributes nothing rather than failing.
        let context = MappingContext::new(json!({
            "request": {"options": {"nativeTools": [{"type": "web_search_20250305"}]}}
        }));
        assert_eq!(
            evaluate_mapping(&mapping, &context, &definitions).unwrap()["tools"],
            json!([{"type": "web_search_20250305"}])
        );

        // Neither source: the whole key is omitted so no empty `tools` array is sent.
        let context = MappingContext::new(json!({"request": {}}));
        assert_eq!(
            evaluate_mapping(&mapping, &context, &definitions).unwrap(),
            json!({})
        );

        // A non-array entry is a manifest mistake, not silently wrapped.
        let context = MappingContext::new(json!({"request": {"options": {"nativeTools": "web"}}}));
        assert!(
            evaluate_mapping(
                &expression(json!({"$concat": [{"$get": "request.options.nativeTools"}]})),
                &context,
                &BTreeMap::new()
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_unknown_operators_and_undeclared_roots() {
        let unknown = expression(json!({"$execute": "rm -rf"}));
        assert!(validate_mapping(&unknown, &BTreeMap::new()).is_err());
        let environment = expression(json!({"$get": "env.OPENAI_API_KEY"}));
        assert!(validate_mapping(&environment, &BTreeMap::new()).is_err());
    }
}
