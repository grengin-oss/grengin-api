// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use connector_server_plugin::{
    CallContext, CompiledConnector, ConnectorError, ConnectorPluginConfig,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use url::Url;

#[derive(Clone, Deserialize)]
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
    #[serde(default)]
    response_pointer: Option<String>,
    #[serde(default)]
    mock_response: Option<Value>,
    #[serde(default)]
    mock_text: Option<String>,
    #[serde(default)]
    mock_mime: Option<String>,
    #[serde(default)]
    expected_content: Option<Value>,
    #[serde(default)]
    query_pairs: Option<Vec<(String, String)>>,
    #[serde(default)]
    forbidden_headers: Vec<String>,
}

#[derive(Default)]
struct Fixture {
    response: Option<Value>,
    text: Option<String>,
    mime: Option<String>,
    captured: Value,
}

async fn respond(
    State(state): State<Arc<Mutex<Fixture>>>,
    OriginalUri(uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let captured = json!({
        "path":uri.path(),"query":uri.query().unwrap_or(""),"method":method.as_str(),
        "headers":headers.iter().map(|(name,value)|(name.as_str(),value.to_str().unwrap())).collect::<BTreeMap<_,_>>(),
        "ok":true,"errors":[],"code":0,
        "authorization":headers.get("authorization").and_then(|v|v.to_str().ok()),
        "prefer":headers.get("prefer").and_then(|v|v.to_str().ok()),
        "body":if body.is_empty(){Value::Null}else{serde_json::from_slice::<Value>(&body).unwrap()},
        "value":[{"id":"fixture"}],"items":[{"id":"fixture"}],
        "nextPageToken":"opaque+/=","@odata.nextLink":"https://graph.microsoft.com/v1.0/next?$skiptoken=opaque"
    });
    let mut state = state.lock().await;
    state.captured = captured.clone();
    if let Some(text) = &state.text {
        (
            [(
                axum::http::header::CONTENT_TYPE,
                state.mime.as_deref().unwrap_or("text/plain"),
            )],
            text.clone(),
        )
            .into_response()
    } else {
        Json(state.response.clone().unwrap_or(captured)).into_response()
    }
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
        &std::fs::read(root.parent().unwrap().join("connector-plugins.json")).unwrap(),
    )
    .unwrap();
    let expected: BTreeSet<_> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["key"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(manifests.keys().cloned().collect::<BTreeSet<_>>(), expected);
    for (id, value) in manifests {
        let setup: Value = serde_json::from_slice(
            &std::fs::read(
                root.join(&id)
                    .join("versions")
                    .join(value["version"].as_str().unwrap())
                    .join("setup.json"),
            )
            .unwrap(),
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
        assert_eq!(
            plugin.manifest().version,
            value["version"].as_str().unwrap()
        );
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
    let fixture = Arc::new(Mutex::new(Fixture::default()));
    let router = Router::new().fallback(respond).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let result = async {
        let manifests = manifests();
        let mut cases: Vec<Case> =
            serde_json::from_str(include_str!("fixtures/provider-requests.json")).unwrap();
        cases.extend(
            serde_json::from_str::<Vec<Case>>(include_str!(
                "fixtures/read-only-expansion-requests.json"
            ))
            .unwrap(),
        );
        cases.extend(
            serde_json::from_str::<Vec<Case>>(include_str!(
                "fixtures/fifty-provider-requests.json"
            ))
            .unwrap(),
        );
        cases.extend(
            serde_json::from_str::<Vec<Case>>(include_str!(
                "fixtures/business-provider-requests.json"
            ))
            .unwrap(),
        );
        let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
        let catalog_fixtures = root
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("scripts/tests/fixtures");
        let google_cases = catalog_fixtures.join("google-expansion-requests.json");
        if google_cases.exists() {
            cases.extend(
                serde_json::from_slice::<Vec<Case>>(&std::fs::read(google_cases).unwrap()).unwrap(),
            );
        }
        let candidate_cases = catalog_fixtures.join("connector-content-requests.json");
        if candidate_cases.exists() {
            let extra: Vec<Case> =
                serde_json::from_slice(&std::fs::read(candidate_cases).unwrap()).unwrap();
            for mut case in extra {
                if let Some(original) = cases.iter().find(|original| original.plugin == case.plugin)
                {
                    case.configuration = original.configuration.clone();
                    case.base_url_override = original.base_url_override.clone();
                }
                cases.push(case);
            }
        }
        // A suite re-exposes its members' tools; each member contract must also hold under the suite's tool name.
        let listing: Value = serde_json::from_slice(
            &std::fs::read(root.parent().unwrap().join("connector-plugins.json")).unwrap(),
        )
        .unwrap();
        for suite in listing.as_array().unwrap() {
            let Some(id) = suite.get("members").and(suite["key"].as_str()) else {
                continue;
            };
            let version = manifests[id]["version"].as_str().unwrap();
            let composition: Value = serde_json::from_slice(
                &std::fs::read(
                    root.join(id)
                        .join("versions")
                        .join(version)
                        .join("composition.json"),
                )
                .unwrap(),
            )
            .unwrap();
            for member in composition["members"].as_array().unwrap() {
                let renames = member["tools"].as_object().unwrap();
                let derived: Vec<Case> = cases
                    .iter()
                    .filter(|case| case.plugin == member["id"].as_str().unwrap())
                    .map(|case| {
                        let mut case = case.clone();
                        case.tool = renames[&case.tool].as_str().unwrap().to_owned();
                        case.plugin = id.to_owned();
                        case
                    })
                    .collect();
                cases.extend(derived);
            }
        }
        let mut covered = BTreeSet::new();
        for case in cases {
            let mut manifest = manifests[&case.plugin].clone();
            // Fixed extra hosts (such as a separate content host) also point at the mock origin.
            if let Some(urls) = manifest
                .get_mut("additional_base_urls")
                .and_then(Value::as_object_mut)
            {
                for url in urls.values_mut() {
                    let path = Url::parse(url.as_str().unwrap())
                        .unwrap()
                        .path()
                        .to_string();
                    *url = json!(format!("{origin}{path}"));
                }
            }
            let base = Url::parse(
                case.base_url_override
                    .as_deref()
                    .unwrap_or(manifest["base_url"].as_str().unwrap()),
            )
            .unwrap();
            let plugin = CompiledConnector::compile(ConnectorPluginConfig {
                manifest,
                auth_mode: None,
                base_url_override: Some(format!("{origin}{}", base.path())),
                configuration: case.configuration.clone(),
                allow_insecure_http: true,
                allow_private_network: true,
            })
            .unwrap();
            for user in ["alice", "bob"] {
                if let Some(mock) = &case.mock_response
                    && let Some(mapping) = &plugin
                        .manifest()
                        .tools
                        .iter()
                        .find(|tool| tool.name == case.tool)
                        .unwrap()
                        .response
                        .result
                {
                    let context = llm_plugin::MappingContext::new(json!({
                        "request":case.args,"config":case.configuration,"session":{"response":mock}
                    }));
                    llm_plugin::evaluate_mapping_bounded(
                        mapping,
                        &context,
                        &plugin.manifest().mappings,
                        50000,
                        64 * 1024 * 1024,
                    )
                    .unwrap_or_else(|error| {
                        panic!("{} {} fixture mapping: {error}", case.plugin, case.tool)
                    });
                }
                *fixture.lock().await = Fixture {
                    response: case.mock_response.clone(),
                    text: case.mock_text.clone(),
                    mime: case.mock_mime.clone(),
                    captured: Value::Null,
                };
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
                if case.plugin == "google-sheets" && case.tool == "get_values_by_filter" {
                    assert_eq!(response["spreadsheetId"], case.args["spreadsheet_id"]);
                    assert_eq!(
                        response["valueRanges"][0]["dataFilters"][0]["a1Range"],
                        case.args["range"]
                    );
                }
                if let Some(expected) = &case.expected_content {
                    assert_eq!(
                        &response["content"], expected,
                        "{} {} content",
                        case.plugin, case.tool
                    );
                    assert_eq!(response["truncated"], false);
                    assert_eq!(response["source_complete"], true);
                }
                let wire = fixture.lock().await.captured.clone();
                let content_pointer = plugin
                    .manifest()
                    .tools
                    .iter()
                    .find(|tool| tool.name == case.tool)
                    .filter(|tool| {
                        tool.response.overflow
                            == connector_server_plugin::ResponseOverflow::Truncate
                    })
                    .map(|_| "/content");
                let response = if case.expected_content.is_some() {
                    &wire
                } else {
                    case.response_pointer.as_deref().or(content_pointer).map_or(
                        &response,
                        |pointer| {
                            response
                                .pointer(pointer)
                                .expect("reviewed response mapping")
                        },
                    )
                };
                assert_eq!(response["path"], case.path, "{} {}", case.plugin, case.tool);
                let query: BTreeMap<String, String> =
                    url::form_urlencoded::parse(response["query"].as_str().unwrap().as_bytes())
                        .into_owned()
                        .collect();
                assert_eq!(query, case.query, "{} {}", case.plugin, case.tool);
                if let Some(expected) = &case.query_pairs {
                    let pairs: Vec<(String, String)> =
                        url::form_urlencoded::parse(response["query"].as_str().unwrap().as_bytes())
                            .into_owned()
                            .collect();
                    assert_eq!(
                        &pairs, expected,
                        "{} {} repeated query",
                        case.plugin, case.tool
                    );
                }
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
                for header in &case.forbidden_headers {
                    assert!(
                        response["headers"][header].is_null(),
                        "{} {} forbidden header",
                        case.plugin,
                        case.tool
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

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn google_calendar_opaque_ids_preserve_fragments_as_data_and_cannot_escape_the_path() {
    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
    let candidate: Value = serde_json::from_slice(
        &std::fs::read(root.join("google-calendar/versions/1.2.1/plugin.json")).unwrap(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Arc::new(Mutex::new(Fixture::default()));
    let router = Router::new().fallback(respond).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let config = |mut manifest: Value| {
        manifest["base_url"] = json!(format!("{origin}/v3"));
        ConnectorPluginConfig {
            manifest,
            allow_insecure_http: true,
            allow_private_network: true,
            ..Default::default()
        }
    };
    let old_manifest = serde_json::from_slice(
        &std::fs::read(root.join("google-calendar/versions/1.2.0/plugin.json")).unwrap(),
    )
    .unwrap();
    let old = CompiledConnector::compile(config(old_manifest)).unwrap();
    let id = "en.fixture#holiday@group.v.calendar.google.com";
    assert!(matches!(
        old.call(
            "list_events",
            json!({"calendar_id":id}),
            CallContext {
                user_id: "fixture-a",
                access_token: Some("fixture-token-a")
            }
        )
        .await,
        Err(ConnectorError::Arguments)
    ));
    assert!(fixture.lock().await.captured.is_null());
    let plugin = CompiledConnector::compile(config(candidate)).unwrap();
    for (calendar, encoded) in [
        ("primary", "primary"),
        (id, "en.fixture%23holiday@group.v.calendar.google.com"),
        ("calendar?other=1", "calendar%3Fother=1"),
        ("calendar%23literal", "calendar%2523literal"),
        ("calendar/part", "calendar%2Fpart"),
    ] {
        for tool in ["list_events", "get_event", "search_events"] {
            for (user, token) in [
                ("fixture-a", "fixture-token-a"),
                ("fixture-b", "fixture-token-b"),
            ] {
                let args = match tool {
                    "get_event" => json!({"calendar_id":calendar,"event_id":"event123"}),
                    "search_events" => json!({"calendar_id":calendar,"query":"fixture"}),
                    _ => json!({"calendar_id":calendar}),
                };
                plugin
                    .call(
                        tool,
                        args,
                        CallContext {
                            user_id: user,
                            access_token: Some(token),
                        },
                    )
                    .await
                    .unwrap();
                let captured = fixture.lock().await.captured.clone();
                let suffix = if tool == "get_event" { "/event123" } else { "" };
                assert_eq!(
                    captured["path"],
                    format!("/v3/calendars/{encoded}/events{suffix}")
                );
                assert_eq!(captured["authorization"], format!("Bearer {token}"));
                assert!(!captured["query"].as_str().unwrap().contains("other=1"));
            }
        }
    }
    let before = fixture.lock().await.captured.clone();
    for calendar in ["", ".", "..", "a/../b", "line\nbreak"] {
        assert!(matches!(
            plugin
                .call(
                    "list_events",
                    json!({"calendar_id":calendar}),
                    CallContext {
                        user_id: "fixture-a",
                        access_token: Some("fixture-token-a")
                    }
                )
                .await,
            Err(ConnectorError::Arguments)
        ));
    }
    assert_eq!(fixture.lock().await.captured, before);
    server.abort();
}

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn google_chat_search_enforces_the_vendor_length_boundary_before_network() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let fixture = Arc::new(Mutex::new(Fixture::default()));
    let router = Router::new().fallback(respond).with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut manifest = manifests().remove("google-chat").unwrap();
    manifest["base_url"] = json!(format!("{origin}/v1"));
    let plugin = CompiledConnector::compile(ConnectorPluginConfig {
        manifest,
        allow_insecure_http: true,
        allow_private_network: true,
        ..Default::default()
    })
    .unwrap();
    for length in [1, 1000] {
        plugin
            .call(
                "search_messages",
                json!({"query":"x".repeat(length)}),
                CallContext {
                    user_id: "test",
                    access_token: Some("fixture"),
                },
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.lock().await.captured["body"]["filter"]
                .as_str()
                .unwrap()
                .len(),
            length
        );
    }
    let before = fixture.lock().await.captured.clone();
    for length in [0, 1001, 2000] {
        assert!(matches!(
            plugin
                .call(
                    "search_messages",
                    json!({"query":"x".repeat(length)}),
                    CallContext {
                        user_id: "test",
                        access_token: Some("fixture")
                    }
                )
                .await,
            Err(ConnectorError::Arguments)
        ));
    }
    assert_eq!(fixture.lock().await.captured, before);
    server.abort();
}

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn new_provider_identifier_and_page_bounds_fail_before_network() {
    let manifests = manifests();
    for (id, tool, args) in [
        (
            "google-contacts",
            "get_contact",
            json!({"contact_id":"people/../other"}),
        ),
        (
            "google-contacts",
            "search_contacts",
            json!({"query":"Ada","page_size":31}),
        ),
        (
            "microsoft-todo",
            "get_task",
            json!({"list_id":"../other","task_id":"123"}),
        ),
        (
            "todoist",
            "get_task",
            json!({"task_id":"tmp-not-yet-synced"}),
        ),
        (
            "miro",
            "list_board_items",
            json!({"board_id":"board123","limit":100}),
        ),
        (
            "miro",
            "list_boards",
            json!({"query":"roadmap","offset":10000}),
        ),
        (
            "calendly",
            "list_event_types",
            json!({"user_id":"https://attacker.invalid/user"}),
        ),
        (
            "typeform",
            "list_responses",
            json!({"form_id":"../private"}),
        ),
    ] {
        let plugin = CompiledConnector::compile(ConnectorPluginConfig {
            manifest: manifests[id].clone(),
            base_url_override: Some("http://127.0.0.1:1".into()),
            allow_insecure_http: true,
            allow_private_network: true,
            ..Default::default()
        })
        .unwrap();
        assert!(
            matches!(
                plugin
                    .call(
                        tool,
                        args,
                        CallContext {
                            user_id: "fixture",
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
                        | "airtable.com"
                        | "www.zoho.com"
                        | "desk.zoho.com"
                        | "www.odoo.com"
                        | "api.sap.com"
                        | "help.sap.com"
                        | "developer.clickup.com"
                        | "developer.monday.com"
                        | "developers.figma.com"
                        | "developers.zoom.us"
                        | "developer.zendesk.com"
                        | "developer.todoist.com"
                        | "developers.miro.com"
                        | "developer.calendly.com"
                        | "www.typeform.com"
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

#[tokio::test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn content_search_rejects_mutations_ungranted_types_and_unsafe_paths_before_network() {
    let manifests = manifests();
    for (id, tool, args, configuration) in [
        (
            "salesforce",
            "soql_query",
            json!({"query":"DELETE FROM Account"}),
            json!({}),
        ),
        (
            "salesforce",
            "soql_query",
            json!({"query":"SELECT Id FROM Account; DELETE FROM Account"}),
            json!({}),
        ),
        (
            "zoho-crm",
            "coql_query",
            json!({"select_query":"UPDATE Contacts SET Last_Name='x'"}),
            json!({}),
        ),
        (
            "github",
            "get_file_contents",
            json!({"owner":"acme","repo":"app","path":["..","secret"]}),
            json!({}),
        ),
        (
            "hubspot",
            "search_objects",
            json!({"object_type":"tickets","query":"private"}),
            json!({}),
        ),
        (
            "odoo",
            "search_read",
            json!({"model":"res.users","fields":["login"]}),
            json!({"database":"demo"}),
        ),
        (
            "odoo",
            "read",
            json!({"model":"res.partner","ids":[1],"fields":["name"],"method":"write"}),
            json!({"database":"demo"}),
        ),
        (
            "airtable",
            "list_records",
            json!({"base_id":"app123","table_id":"tbl123","sort_direction":"asc"}),
            json!({}),
        ),
        (
            "notion",
            "query_data_source",
            json!({"data_source_id":"source-1","sorts":[{"property":"Name","timestamp":"created_time","direction":"ascending"}]}),
            json!({}),
        ),
        (
            "wix",
            "query_data_items",
            json!({"site_id":"site\nheader","data_collection_id":"Products"}),
            json!({"account_id":"account-1"}),
        ),
    ] {
        let plugin = CompiledConnector::compile(ConnectorPluginConfig {
            manifest: manifests[id].clone(),
            configuration,
            base_url_override: Some("http://127.0.0.1:1".into()),
            allow_insecure_http: true,
            allow_private_network: true,
            ..Default::default()
        })
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
            "{id}/{tool}"
        );
    }
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
        let plugin = CompiledConnector::compile(ConnectorPluginConfig {
            manifest: manifests[id].clone(),
            base_url_override: Some("http://127.0.0.1:1".into()),
            configuration: if id == "odoo" {
                json!({"database":"grengin_demo"})
            } else {
                Value::Null
            },
            allow_insecure_http: true,
            allow_private_network: true,
            ..Default::default()
        })
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
