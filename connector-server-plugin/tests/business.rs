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

fn local_config(manifest: Value, base: String, configuration: Value) -> ConnectorPluginConfig {
    let mut config = ConnectorPluginConfig {
        manifest,
        base_url_override: Some(base),
        configuration,
        ..Default::default()
    };
    config.allow_insecure_http = true;
    config.allow_private_network = true;
    config
}

fn manifests() -> BTreeMap<String, Value> {
    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").expect("set GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR to grengin-list/master-data/connector-plugins"));
    let all: BTreeMap<String, Value> = std::fs::read_dir(root)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path().join("plugin.json");
            path.is_file().then(|| {
                let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                (value["id"].as_str().unwrap().to_owned(), value)
            })
        })
        .collect();
    all.into_iter()
        .filter(|(id, _)| {
            matches!(
                id.as_str(),
                "zoho-crm" | "zoho-books" | "zoho-desk" | "odoo" | "sap-s4hana"
            )
        })
        .collect()
}

#[test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
fn business_manifests_validate_and_have_no_embedded_credentials() {
    let schema: Value =
        serde_json::from_str(include_str!("../schema/connector-plugin-v1.schema.json")).unwrap();
    let manifests = manifests();
    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
    let catalog: Value = serde_json::from_slice(
        &std::fs::read(root.parent().unwrap().join("connector-plugins.json")).unwrap(),
    )
    .unwrap();
    let expected: BTreeSet<_> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap().to_owned())
        .filter(|id| {
            matches!(
                id.as_str(),
                "zoho-crm" | "zoho-books" | "zoho-desk" | "odoo" | "sap-s4hana"
            )
        })
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
async fn every_business_tool_obeys_its_http_contract_and_preserves_page_cursors() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router=Router::new().fallback(|OriginalUri(uri):OriginalUri,method:Method,headers:HeaderMap,body:Bytes| async move {
        Json(json!({
            "path":uri.path(),"query":uri.query().unwrap_or(""),"method":method.as_str(),
            "headers":headers.iter().map(|(name,value)|(name.as_str(),value.to_str().unwrap())).collect::<BTreeMap<_,_>>(),
            "ok":true,"errors":[],"code":0,
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
            serde_json::from_str(include_str!("fixtures/business-provider-requests.json")).unwrap();
        let mut covered = BTreeSet::new();
        for case in cases {
            let manifest = manifests[&case.plugin].clone();
            let base = Url::parse(
                case.base_url_override
                    .as_deref()
                    .unwrap_or(manifest["base_url"].as_str().unwrap()),
            )
            .unwrap();
            let plugin = CompiledConnector::compile(local_config(
                manifest,
                format!("{origin}{}", base.path()),
                case.configuration.clone(),
            ))
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

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn business_arguments_cannot_change_hosts_organizations_or_read_methods() {
    let manifests = manifests();
    for (id, tool, args) in [
        ("zoho-crm", "get_lead", json!({"record_id":"../Contacts"})),
        (
            "zoho-crm",
            "get_module_fields",
            json!({"module":"../users"}),
        ),
        (
            "zoho-crm",
            "list_contacts",
            json!({"page":1,"page_token":"opaque"}),
        ),
        ("zoho-crm", "list_contacts", json!({"per_page":201})),
        ("zoho-books", "list_invoices", json!({})),
        (
            "zoho-books",
            "get_invoice",
            json!({"organization_id":"123","record_id":"1?organization_id=456"}),
        ),
        (
            "zoho-desk",
            "list_tickets",
            json!({"organization_id":"123\r\nHost: attacker.example"}),
        ),
        (
            "zoho-desk",
            "list_tickets",
            json!({"organization_id":"123","limit":101}),
        ),
        ("odoo", "get_contact", json!({"record_id":0})),
        (
            "odoo",
            "get_contact",
            json!({"record_id":1,"method":"unlink"}),
        ),
        ("odoo", "list_contacts", json!({"limit":101})),
        ("sap-s4hana", "list_customers", json!({"top":101})),
        (
            "sap-s4hana",
            "list_customers",
            json!({"query":"$expand=everything"}),
        ),
    ] {
        let plugin = CompiledConnector::compile(local_config(
            manifests[id].clone(),
            "http://127.0.0.1:1".into(),
            if id == "odoo" {
                json!({"database":"grengin_demo"})
            } else {
                Value::Null
            },
        ))
        .unwrap();
        assert!(
            matches!(
                plugin
                    .call(
                        tool,
                        args,
                        CallContext {
                            user_id: "alice",
                            access_token: Some("fixture")
                        }
                    )
                    .await,
                Err(ConnectorError::Arguments)
            ),
            "{id} {tool}"
        );
    }
}

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn business_empty_and_provider_error_responses_are_not_authentication_successes() {
    use axum::http::StatusCode;
    let manifests = manifests();
    for (id, tool, status, response, success) in [
        (
            "zoho-crm",
            "list_contacts",
            StatusCode::NO_CONTENT,
            Value::Null,
            true,
        ),
        (
            "zoho-books",
            "list_organizations",
            StatusCode::OK,
            json!({"code":0,"organizations":[]}),
            true,
        ),
        (
            "zoho-books",
            "list_organizations",
            StatusCode::OK,
            json!({"code":57,"message":"vendor diagnostic"}),
            false,
        ),
        (
            "zoho-books",
            "list_organizations",
            StatusCode::OK,
            json!({"organizations":[]}),
            false,
        ),
        (
            "zoho-desk",
            "list_organizations",
            StatusCode::UNAUTHORIZED,
            json!({"errorCode":"INVALID_OAUTH"}),
            false,
        ),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let body = response.clone();
        let router = Router::new().fallback(move || {
            let body = body.clone();
            async move { (status, Json(body)) }
        });
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let plugin = CompiledConnector::compile(ConnectorPluginConfig {
            manifest: manifests[id].clone(),
            base_url_override: Some(origin),
            allow_insecure_http: true,
            allow_private_network: true,
            ..Default::default()
        })
        .unwrap();
        let result = plugin
            .call(
                tool,
                json!({}),
                CallContext {
                    user_id: "alice",
                    access_token: Some("fixture"),
                },
            )
            .await;
        server.abort();
        let _ = server.await;
        if success {
            assert_eq!(result.unwrap(), response);
        } else {
            let error = result.unwrap_err();
            assert!(matches!(
                error,
                ConnectorError::ProviderRejected | ConnectorError::Http { .. }
            ));
            assert!(!error.to_string().contains("vendor diagnostic"));
        }
    }
}

#[test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
fn business_self_hosted_public_hosts_need_explicit_policy_and_override() {
    let mut value = manifests()["odoo"].clone();
    value["base_url_policy"] = json!({
        "allowed_host_suffixes": [], "required_override": true, "allow_custom_public_host": true
    });
    let mut setup = ConnectorPluginConfig {
        manifest: value.clone(),
        configuration: json!({"database":"demo"}),
        ..Default::default()
    };
    setup.allow_private_network = false;
    setup.allow_insecure_http = false;
    assert!(
        CompiledConnector::compile(setup.clone())
            .unwrap()
            .validate_configuration()
            .is_err()
    );
    setup.base_url_override = Some("https://customer.example/json/2".into());
    assert!(
        CompiledConnector::compile(setup.clone())
            .unwrap()
            .validate_configuration()
            .is_ok()
    );
    for url in [
        "http://customer.example/json/2",
        "https://127.0.0.1/json/2",
        "https://169.254.169.254/latest",
        "https://user:secret@customer.example/json/2",
    ] {
        setup.base_url_override = Some(url.into());
        assert!(CompiledConnector::compile(setup.clone()).is_err(), "{url}");
    }
    for policy in [
        json!({"allowed_host_suffixes": [], "required_override": true}),
        json!({"allowed_host_suffixes": [], "allow_custom_public_host": true}),
        json!({"allowed_host_suffixes": [".example.com"], "required_override": true, "allow_custom_public_host": true}),
    ] {
        value["base_url_policy"] = policy;
        assert!(
            CompiledConnector::compile(ConnectorPluginConfig {
                manifest: value.clone(),
                configuration: json!({"database":"demo"}),
                ..Default::default()
            })
            .is_err()
        );
    }
}
