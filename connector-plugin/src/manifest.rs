// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use llm_plugin::MappingExpression;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorManifest {
    pub schema_version: u32,
    pub id: String,
    pub version: String,
    pub name: String,
    pub base_url: String,
    pub auth: ConnectorAuth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<ConnectorPresentation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_delivery: Option<CredentialDelivery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_policy: Option<BaseUrlPolicy>,
    /// Fixed extra origins a tool can target by name, such as a vendor's separate content host.
    /// Credentials are sent to them, so they are part of the reviewed manifest and never
    /// administrator overrides.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub additional_base_urls: BTreeMap<String, String>,
    #[serde(default)]
    pub tools: Vec<ConnectorTool>,
    #[serde(default)]
    pub mappings: BTreeMap<String, MappingExpression>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConnectorToolGroups {
    pub read_only: Vec<String>,
    pub read_write: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorPresentation {
    #[serde(default)]
    pub suggested_prompts: Vec<String>,
    /// Topic groups that let administrators switch related tools together. When present,
    /// every tool belongs to exactly one group. Access is still enforced per tool.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topic_groups: Vec<ConnectorTopicGroup>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectorTopicGroup {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub tools: Vec<String>,
}

const MAX_TOPIC_GROUPS: usize = 32;

impl ConnectorManifest {
    pub fn tool_groups(&self) -> ConnectorToolGroups {
        let mut groups = ConnectorToolGroups::default();
        for tool in &self.tools {
            if tool.read_only {
                groups.read_only.push(tool.name.clone());
            } else {
                groups.read_write.push(tool.name.clone());
            }
        }
        groups
    }

    pub fn topic_groups(&self) -> &[ConnectorTopicGroup] {
        self.presentation
            .as_ref()
            .map(|presentation| presentation.topic_groups.as_slice())
            .unwrap_or_default()
    }

    pub(crate) fn validate_topic_groups(&self) -> Result<(), &'static str> {
        let groups = self.topic_groups();
        if groups.is_empty() {
            return Ok(());
        }
        if groups.len() > MAX_TOPIC_GROUPS {
            return Err("too many topic groups");
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut owners = BTreeMap::new();
        for group in groups {
            if !topic_group_id(&group.id) || !ids.insert(group.id.as_str()) {
                return Err("topic group id");
            }
            if !display_text(&group.name, 60)
                || group
                    .description
                    .as_ref()
                    .is_some_and(|text| !display_text(text, 240))
            {
                return Err("topic group text");
            }
            if group.tools.is_empty() {
                return Err("topic group without tools");
            }
            for tool in &group.tools {
                if owners.insert(tool.as_str(), group.id.as_str()).is_some() {
                    return Err("tool in more than one topic group");
                }
            }
        }
        if owners.len() != self.tools.len()
            || self
                .tools
                .iter()
                .any(|tool| !owners.contains_key(tool.name.as_str()))
        {
            return Err("topic groups must cover every tool exactly once");
        }
        Ok(())
    }
}

fn topic_group_id(value: &str) -> bool {
    let mut chars = value.chars();
    value.len() <= 64
        && chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

fn display_text(value: &str, max_chars: usize) -> bool {
    value.trim() == value
        && !value.is_empty()
        && value.chars().count() <= max_chars
        && !value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAuth {
    None,
    Oauth2,
    ApiKey,
}

impl ConnectorAuth {
    pub fn resolve_mode(self, mode: Option<Self>) -> Option<Self> {
        let selected = mode.unwrap_or(self);
        (selected == self || (self == Self::Oauth2 && selected == Self::ApiKey)).then_some(selected)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "location", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialDelivery {
    Header {
        name: String,
        prefix: Option<String>,
    },
    JsonBody {
        field: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BaseUrlPolicy {
    pub allowed_host_suffixes: Vec<String>,
    #[serde(default)]
    pub required_override: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_custom_public_host: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    #[serde(default)]
    pub destructive: bool,
    #[serde(default)]
    pub required_scopes: Vec<String>,
    pub request: ConnectorRequest,
    #[serde(default)]
    pub response: ConnectorResponse,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorRequest {
    pub method: HttpMethod,
    pub path: String,
    #[serde(default)]
    pub path_parameters: BTreeMap<String, MappingExpression>,
    #[serde(default)]
    pub query: BTreeMap<String, MappingExpression>,
    #[serde(default)]
    pub headers: BTreeMap<String, MappingExpression>,
    #[serde(default)]
    pub body: Option<MappingExpression>,
    /// Name of an `additional_base_urls` entry to send this request to instead of `base_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub path_parameter_encoding: BTreeMap<String, PathParameterEncoding>,
}

/// How a whole-segment path parameter value is placed in the URL.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PathParameterEncoding {
    /// The value must be a plain segment: no `/`, `%`, `?`, `#` or `\\`.
    #[default]
    Segment,
    /// An opaque identifier that may contain reserved characters; they are percent-encoded once.
    Opaque,
    /// Opaque, but double-encoded when the value starts with `/` or contains `//`, which is how
    /// Zoom requires such meeting UUIDs to be sent.
    OpaqueDoubleIfSlash,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorResponse {
    #[serde(default)]
    pub format: ResponseFormat,
    #[serde(default)]
    pub overflow: ResponseOverflow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_source_bytes: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_result_bytes: Option<usize>,
    #[serde(default)]
    pub result: Option<MappingExpression>,
    #[serde(default)]
    pub error_if: Option<MappingExpression>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_array_pointer: Option<String>,
    #[serde(default)]
    pub pagination: Option<NextLinkPagination>,
    /// Text tools only: also accept `application/octet-stream` (or no content type), which many
    /// download endpoints send for every file. The bytes must still be UTF-8 without NULs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accept_octet_stream: bool,
    /// Follow a GET's redirect to a pre-signed download URL. Hops never carry credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirects: Option<RedirectPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedirectPolicy {
    /// Exact hosts, or `.suffix` entries matching any subdomain.
    pub allowed_hosts: Vec<String>,
    pub max_hops: u8,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    #[default]
    Json,
    Text,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResponseOverflow {
    #[default]
    Error,
    Truncate,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NextLinkPagination {
    pub items_pointer: String,
    pub next_pointer: String,
    pub max_pages: u16,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorPluginConfig {
    pub manifest: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<ConnectorAuth>,
    #[serde(default)]
    pub base_url_override: Option<String>,
    #[serde(default)]
    pub configuration: Value,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub allow_private_network: bool,
}
