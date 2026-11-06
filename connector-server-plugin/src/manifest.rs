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
    pub credential_delivery: Option<CredentialDelivery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url_policy: Option<BaseUrlPolicy>,
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
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorAuth {
    None,
    Oauth2,
    ApiKey,
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
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectorResponse {
    #[serde(default)]
    pub result: Option<MappingExpression>,
    #[serde(default)]
    pub error_if: Option<MappingExpression>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_array_pointer: Option<String>,
    #[serde(default)]
    pub pagination: Option<NextLinkPagination>,
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
    #[serde(default)]
    pub base_url_override: Option<String>,
    #[serde(default)]
    pub configuration: Value,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub allow_private_network: bool,
}
