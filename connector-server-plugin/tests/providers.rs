// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::OriginalUri,
    http::{HeaderMap, Method},
};
use connector_server_plugin::{
    CallContext, CompiledConnector, ConnectorError, ConnectorPluginConfig,
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

#[derive(Deserialize)]
struct Case {
    plugin: String,
    tool: String,
    args: Value,
    path: String,
    query: BTreeMap<String, String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default)]
    configuration: Value,
    #[serde(default)]
    base_url_override: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

fn manifests() -> BTreeMap<String, Value> {
    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").expect("set GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR to grengin-list/master-data/connector-plugins"));
    std::fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path().join("plugin.json");
            path.is_file().then(|| {
                let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                (value["id"].as_str().unwrap().to_owned(), value)
            })
        })
        .collect()
}

#[test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
fn all_provider_manifests_validate_and_have_no_embedded_credentials() {
    let schema: Value =
        serde_json::from_str(include_str!("../schema/connector-plugin-v1.schema.json")).unwrap();
    let manifests = manifests();
    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
    let catalog: Value = serde_json::from_slice(
        &std::fs::read(root.parent().unwrap().join("connectors.json")).unwrap(),
    )
    .unwrap();
    let expected: BTreeSet<_> = catalog["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(manifests.keys().cloned().collect::<BTreeSet<_>>(), expected);
    for (id, value) in manifests {
        let setup: Value = serde_json::from_slice(
            &std::fs::read(root.join(&id).join("versions/1.0.0/setup.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(setup["plugin_id"], id);
        assert_eq!(setup["plugin_version"], value["version"]);
        assert_eq!(setup["authentication"]["kind"], value["auth"]);
        assert!(jsonschema::is_valid(&schema, &value), "{id}");
        let plugin = CompiledConnector::compile(ConnectorPluginConfig {
            manifest: value.clone(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(plugin.manifest().version, "1.0.0");
        assert!(
            plugin
                .manifest()
                .tools
                .iter()
                .all(|t| t.read_only && !t.destructive)
        );
        let text = serde_json::to_string(&value).unwrap();
        for forbidden in [
            "client_secret",
            "access_token",
            "refresh_token",
            "Bearer ",
            "grengin_connector_key",
        ] {
            assert!(!text.contains(forbidden), "{id}: {forbidden}");
        }
    }
}

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn every_provider_tool_obeys_its_http_contract_and_preserves_page_cursors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router=Router::new().fallback(|OriginalUri(uri):OriginalUri,method:Method,headers:HeaderMap,body:Bytes| async move {
        Json(json!({
            "path":uri.path(),"query":uri.query().unwrap_or(""),"method":method.as_str(),
            "headers":headers.iter().map(|(name,value)|(name.as_str(),value.to_str().unwrap())).collect::<BTreeMap<_,_>>(),
            "ok":true,"errors":[],
            "authorization":headers.get("authorization").and_then(|v|v.to_str().ok()),
            "prefer":headers.get("prefer").and_then(|v|v.to_str().ok()),
            "body":if body.is_empty(){Value::Null}else{serde_json::from_slice::<Value>(&body).unwrap()},
            "value":[{"id":"fixture"}],"items":[{"id":"fixture"}],
            "nextPageToken":"opaque+/=","@odata.nextLink":"https://graph.microsoft.com/v1.0/next?$skiptoken=opaque"
        }))
    });
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = async {
        let manifests = manifests();
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("fixtures/provider-requests.json")).unwrap();
        let mut covered = BTreeSet::new();
        for case in cases {
            let manifest = manifests[&case.plugin].clone();
            let base = Url::parse(
                case.base_url_override
                    .as_deref()
                    .unwrap_or(manifest["base_url"].as_str().unwrap()),
            )
            .unwrap();
            let plugin = CompiledConnector::compile(ConnectorPluginConfig {
                manifest,
                base_url_override: Some(format!("{origin}{}", base.path())),
                configuration: case.configuration.clone(),
                allow_insecure_http: true,
                allow_private_network: true,
            })
            .unwrap();
            for user in ["alice", "bob"] {
                let token = format!("fixture-{user}");
                let response = plugin
                    .call(
                        &case.tool,
                        case.args.clone(),
                        CallContext {
                            user_id: user,
                            access_token: Some(&token),
                        },
                    )
                    .await
                    .unwrap_or_else(|error| panic!("{} {}: {error}", case.plugin, case.tool));
                assert_eq!(response["path"], case.path, "{} {}", case.plugin, case.tool);
                let query: BTreeMap<String, String> =
                    url::form_urlencoded::parse(response["query"].as_str().unwrap().as_bytes())
                        .into_owned()
                        .collect();
                assert_eq!(query, case.query, "{} {}", case.plugin, case.tool);
                assert_eq!(response["method"], case.method.as_deref().unwrap_or("GET"));
                let mut expected_body = case.body.clone().unwrap_or(Value::Null);
                match &plugin.manifest().credential_delivery {
                    Some(connector_server_plugin::CredentialDelivery::JsonBody { field }) => {
                        if expected_body.is_null() {
                            expected_body = json!({});
                        }
                        expected_body[field] = json!(token);
                        assert!(response["authorization"].is_null());
                    }
                    Some(connector_server_plugin::CredentialDelivery::Header { name, prefix }) => {
                        assert_eq!(
                            response["headers"][name.to_ascii_lowercase()],
                            format!("{}{token}", prefix.as_deref().unwrap_or_default())
                        );
                        if !name.eq_ignore_ascii_case("authorization") {
                            assert!(response["authorization"].is_null());
                        }
                    }
                    None => assert_eq!(response["authorization"], format!("Bearer {token}")),
                }
                assert_eq!(
                    response["body"], expected_body,
                    "{} {} body",
                    case.plugin, case.tool
                );
                for (header, value) in &case.headers {
                    assert_eq!(
                        response["headers"][header], *value,
                        "{} {} header",
                        case.plugin, case.tool
                    );
                }
                if case.plugin == "outlook-mail" && case.tool == "get_message" {
                    assert_eq!(response["prefer"], "outlook.body-content-type=\"text\"");
                }
                assert_eq!(response["nextPageToken"], "opaque+/=");
                assert!(response["@odata.nextLink"].is_string());
            }
            assert!(matches!(
                plugin
                    .call(
                        &case.tool,
                        case.args.clone(),
                        CallContext {
                            user_id: "alice",
                            access_token: None
                        }
                    )
                    .await,
                Err(ConnectorError::Authentication)
            ));
            let mut invalid = case.args.clone();
            invalid["unexpected_argument"] = json!(true);
            assert!(matches!(
                plugin
                    .call(
                        &case.tool,
                        invalid,
                        CallContext {
                            user_id: "alice",
                            access_token: Some("fixture")
                        }
                    )
                    .await,
                Err(ConnectorError::Arguments)
            ));
            covered.insert((case.plugin, case.tool));
        }
        for (id, manifest) in manifests {
            for tool in manifest["tools"].as_array().unwrap() {
                assert!(
                    covered.contains(&(id.clone(), tool["name"].as_str().unwrap().into())),
                    "untested tool in {id}"
                );
            }
        }
    }
    .await;
    server.abort();
    result
}

#[test]
fn tool_categories_follow_annotations_not_http_verbs() {
    let reference: Value = serde_json::from_str(include_str!("../example.json")).unwrap();
    let plugin = CompiledConnector::compile(ConnectorPluginConfig {
        manifest: reference.clone(),
        ..Default::default()
    })
    .unwrap();
    let groups = plugin.manifest().tool_groups();
    assert_eq!(
        groups.read_only,
        vec!["list_mail", "get_mail", "list_drive_files"]
    );
    assert_eq!(groups.read_write, vec!["create_mail_draft"]);
    let mut calendar = reference;
    calendar["tools"][3]["name"] = json!("query_free_busy");
    calendar["tools"][3]["read_only"] = json!(true);
    calendar["tools"][3]["request"]["path"] = json!("/availability");
    let plugin = CompiledConnector::compile(ConnectorPluginConfig {
        manifest: calendar,
        ..Default::default()
    })
    .unwrap();
    assert!(
        plugin
            .manifest()
            .tool_groups()
            .read_only
            .contains(&"query_free_busy".into())
    );
    assert!(plugin.manifest().tool_groups().read_write.is_empty());
    assert!(
        plugin
            .manifest()
            .tools
            .iter()
            .any(|t| t.name == "query_free_busy" && t.request.method.as_str() == "POST")
    );
}

#[test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
fn reviewed_tool_inventory_matches_manifests_scopes_and_categories() {
    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
    let coverage: Value =
        serde_json::from_slice(&std::fs::read(root.join("coverage.json")).unwrap()).unwrap();
    let manifests = manifests();
    let providers = coverage["providers"].as_array().unwrap();
    assert_eq!(providers.len(), manifests.len());
    let mut names = BTreeSet::new();
    for provider in providers {
        let id = provider["id"].as_str().unwrap();
        assert!(names.insert(id));
        let plugin = CompiledConnector::compile(ConnectorPluginConfig {
            manifest: manifests[id].clone(),
            ..Default::default()
        })
        .unwrap();
        let groups = plugin.manifest().tool_groups();
        assert_eq!(
            provider["tool_groups"],
            serde_json::to_value(&groups).unwrap(),
            "{id}"
        );
        let tools = provider["tools"].as_array().unwrap();
        assert_eq!(tools.len(), plugin.manifest().tools.len());
        let mut documented = BTreeSet::new();
        for tool in tools {
            let name = tool["name"].as_str().unwrap();
            assert!(documented.insert(name));
            let definition = plugin
                .manifest()
                .tools
                .iter()
                .find(|t| t.name == name)
                .unwrap();
            assert_eq!(tool["method"], definition.request.method.as_str());
            assert_eq!(tool["path"], definition.request.path);
            assert_eq!(tool["required_scopes"], json!(definition.required_scopes));
            let source = Url::parse(tool["source"].as_str().unwrap()).unwrap();
            assert_eq!(source.scheme(), "https");
            assert!(matches!(
                source.host_str(),
                Some(
                    "learn.microsoft.com"
                        | "developers.google.com"
                        | "developers.asana.com"
                        | "developer.box.com"
                        | "www.canva.dev"
                        | "developer.atlassian.com"
                        | "docs.dropboxapi.com"
                        | "docs.github.com"
                        | "developers.hubspot.com"
                        | "developers.intercom.com"
                        | "linear.app"
                        | "mailchimp.com"
                        | "developers.notion.com"
                        | "developer.paypal.com"
                        | "developer.intuit.com"
                        | "developer.salesforce.com"
                        | "docs.sentry.io"
                        | "shopify.dev"
                        | "docs.slack.dev"
                        | "developer.squareup.com"
                        | "docs.stripe.com"
                        | "supabase.com"
                        | "developers.webflow.com"
                        | "dev.wix.com"
                        | "developer.wordpress.com"
                        | "github.com"
                )
            ));
        }
        let scopes: BTreeSet<_> = plugin
            .manifest()
            .tools
            .iter()
            .flat_map(|t| t.required_scopes.iter().cloned())
            .collect();
        assert_eq!(provider["required_scopes"], json!(scopes));
        for (category, read_only) in [("read_only", true), ("read_write", false)] {
            let mut listed = BTreeSet::new();
            for entry in provider["catalog_tools"][category].as_array().unwrap() {
                assert!(listed.insert(entry["name"].as_str().unwrap()));
                let mapped = entry["plugin_tools"].as_array().unwrap();
                let status = entry["status"].as_str().unwrap();
                assert!(matches!(
                    status,
                    "scenario_equivalent" | "partial" | "deferred"
                ));
                assert_eq!(status == "deferred", mapped.is_empty());
                assert!(!entry["notes"].as_str().unwrap().is_empty());
                for name in mapped {
                    let tool = plugin
                        .manifest()
                        .tools
                        .iter()
                        .find(|t| t.name == name.as_str().unwrap())
                        .unwrap();
                    assert_eq!(tool.read_only, read_only);
                }
            }
        }
    }
}
