// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::mcp_servers::McpTransportType;
use connector_server_plugin::{
    CompiledConnector, ConnectorAuth, ConnectorError, ConnectorPluginConfig,
};
use rmcp::model::{Tool, ToolAnnotations};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct PluginOAuthMetadata {
    auth_url: String,
    token_url: String,
    refresh_token_url: Option<String>,
    redirect_url: String,
    scopes: Vec<String>,
}

pub fn compile_plugin(
    config: &Value,
    transport: McpTransportType,
    url: Option<&str>,
) -> Result<Option<CompiledConnector>, ConnectorError> {
    let Some(raw) = config.get("connector_plugin") else {
        return Ok(None);
    };
    if transport != McpTransportType::Http {
        return Err(ConnectorError::Manifest(
            "connector plugins require http transport",
        ));
    }
    // Custom manifests cannot borrow a catalog connector's centrally approved OAuth identity.
    if config.as_object().is_none_or(|map| {
        map.keys()
            .any(|key| !matches!(key.as_str(), "connector_plugin" | "oauth"))
    }) {
        return Err(ConnectorError::Manifest("mixed connector configuration"));
    }
    let plugin_config: ConnectorPluginConfig = serde_json::from_value(raw.clone())
        .map_err(|_| ConnectorError::Manifest("connector_plugin"))?;
    if let Some(oauth) = config.get("oauth") {
        let oauth: PluginOAuthMetadata = serde_json::from_value(oauth.clone()).map_err(|_| {
            ConnectorError::Manifest("OAuth endpoints, redirect_url and scopes required")
        })?;
        for raw in [&oauth.auth_url, &oauth.token_url]
            .into_iter()
            .chain(oauth.refresh_token_url.iter())
        {
            let url = url::Url::parse(raw)
                .map_err(|_| ConnectorError::Manifest("invalid OAuth endpoint"))?;
            connector_server_plugin::validate_endpoint_url(
                &url,
                plugin_config.allow_insecure_http,
                plugin_config.allow_private_network,
            )?;
        }
        let redirect = url::Url::parse(&oauth.redirect_url)
            .map_err(|_| ConnectorError::Manifest("invalid OAuth redirect"))?;
        let loopback = matches!(
            redirect.host_str(),
            Some("localhost" | "127.0.0.1" | "[::1]")
        );
        if (redirect.scheme() != "https" && !(redirect.scheme() == "http" && loopback))
            || !redirect.username().is_empty()
            || redirect.password().is_some()
            || redirect.fragment().is_some()
            || oauth.scopes.len() > 64
            || oauth.scopes.iter().any(|s| {
                s.is_empty()
                    || s.len() > 256
                    || s.chars().any(|c| c.is_whitespace() || c.is_control())
            })
        {
            return Err(ConnectorError::Manifest("invalid OAuth redirect or scopes"));
        }
    }
    let plugin = CompiledConnector::compile(plugin_config)?;
    plugin.validate_configuration()?;
    if url.and_then(|s| url::Url::parse(s).ok()) != Some(plugin.base_url().clone()) {
        return Err(ConnectorError::Manifest(
            "server URL must match plugin base_url",
        ));
    }
    let oauth = config.get("oauth");
    if (plugin.manifest().auth == ConnectorAuth::Oauth2 && !oauth.is_some_and(Value::is_object))
        || (plugin.manifest().auth != ConnectorAuth::Oauth2 && oauth.is_some())
    {
        return Err(ConnectorError::Manifest(
            "OAuth configuration must match plugin auth",
        ));
    }
    Ok(Some(plugin))
}

pub fn plugin_tools(plugin: &CompiledConnector) -> Vec<Tool> {
    plugin
        .manifest()
        .tools
        .iter()
        .map(|tool| {
            Tool::new(
                tool.name.clone(),
                tool.description.clone(),
                tool.input_schema.as_object().cloned().unwrap_or_default(),
            )
            .with_annotations(
                ToolAnnotations::new()
                    .read_only(tool.read_only)
                    .destructive(tool.destructive),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn oauth_scopes_and_destinations_fail_closed_before_installation() {
        let manifest: Value =
            serde_json::from_str(include_str!("../../connector-server-plugin/example.json"))
                .unwrap();
        let config = json!({"connector_plugin":{"manifest":manifest},"oauth":{"auth_url":"https://identity.example/authorize","token_url":"https://identity.example/token","redirect_url":"http://localhost:5173/mcp/oauth/callback","scopes":["Mail.Read"]}});
        assert!(
            compile_plugin(
                &config,
                McpTransportType::Http,
                Some("https://graph.microsoft.com/v1.0")
            )
            .is_ok()
        );
        for bad in ["scope\u{0}", "scope\n", "two scopes"] {
            let mut invalid = config.clone();
            invalid["oauth"]["scopes"] = json!([bad]);
            assert!(
                compile_plugin(
                    &invalid,
                    McpTransportType::Http,
                    Some("https://graph.microsoft.com/v1.0")
                )
                .is_err()
            );
        }
        let mut invalid = config.clone();
        invalid["oauth"]["token_url"] = json!("https://169.254.169.254/token");
        assert!(
            compile_plugin(
                &invalid,
                McpTransportType::Http,
                Some("https://graph.microsoft.com/v1.0")
            )
            .is_err()
        );
        let mut invalid = config;
        invalid["grengin_connector_key"] = json!("slack");
        assert!(
            compile_plugin(
                &invalid,
                McpTransportType::Http,
                Some("https://graph.microsoft.com/v1.0")
            )
            .is_err()
        );
    }
}
