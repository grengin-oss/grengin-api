// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Compile production transport/config code without the backend's unrelated DB and handler graph.
#[path = "../../src/services/http_client.rs"]
pub mod http_client;
#[path = "../../src/services/mcp_client.rs"]
pub mod mcp_client;
#[path = "../../src/services/mcp_plugin_config.rs"]
pub mod mcp_plugin_config;

pub mod services {
    pub use super::http_client;
    pub use super::mcp_plugin_config as mcp_plugin_helpers;
}

pub mod models {
    pub mod mcp_servers {
        // Only the DB transport enum is a fixture; OAuth, sessions, HTTP and config use real code.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
        #[serde(rename_all = "lowercase")]
        pub enum McpTransportType {
            Stdio,
            Http,
            Sse,
            Websocket,
        }
    }
}

#[test]
#[ignore = "cross-repository catalog test; requires GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
fn business_regional_setups_pass_production_installation_and_scope_encoding() {
    use mcp_client::{build_authorization_url, oauth_config_from_connection};
    use models::mcp_servers::McpTransportType;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::path::PathBuf;

    let root = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
    for id in ["zoho-crm", "zoho-books", "zoho-desk"] {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(root.join(id).join("plugin.json")).unwrap())
                .unwrap();
        let setup: Value = serde_json::from_slice(
            &std::fs::read(root.join(id).join("versions/1.0.0/setup.json")).unwrap(),
        )
        .unwrap();
        let profiles = setup["connection_profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 3);
        for profile in profiles {
            let mut oauth = setup["authentication"]["oauth"].clone();
            oauth["auth_url"] = profile["auth_url"].clone();
            oauth["token_url"] = profile["token_url"].clone();
            oauth["redirect_url"] = json!("http://localhost:5173/mcp/oauth/callback");
            let config = json!({
                "connector_plugin": {"manifest": manifest, "base_url_override": profile["base_url_override"], "configuration": {}},
                "oauth": oauth
            });
            mcp_plugin_config::compile_plugin(
                &config,
                McpTransportType::Http,
                profile["base_url_override"].as_str(),
            )
            .unwrap()
            .unwrap();
            let parsed = oauth_config_from_connection(
                &config,
                "fixture-client",
                Some("fixture-secret".into()),
            )
            .unwrap();
            let auth = build_authorization_url(&parsed).unwrap();
            let url = url::Url::parse(&auth.authorization_url).unwrap();
            let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(
                url.origin().ascii_serialization(),
                url::Url::parse(profile["auth_url"].as_str().unwrap())
                    .unwrap()
                    .origin()
                    .ascii_serialization()
            );
            assert_eq!(query["scope"], parsed.scopes.join(","));
            assert_eq!(query["access_type"], "offline");
            assert_eq!(query["prompt"], "consent");
            assert_eq!(
                query["redirect_uri"],
                "http://localhost:5173/mcp/oauth/callback"
            );
            assert!(!query.contains_key("client_secret"));
        }
    }
    for (id, base, configuration) in [
        (
            "odoo",
            "https://customer.example/json/2",
            json!({"database":"grengin_demo"}),
        ),
        (
            "sap-s4hana",
            "https://customer-api.s4hana.ondemand.com/sap/opu",
            json!({}),
        ),
    ] {
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(root.join(id).join("plugin.json")).unwrap())
                .unwrap();
        let config = json!({"connector_plugin":{"manifest":manifest,"base_url_override":base,"configuration":configuration}});
        mcp_plugin_config::compile_plugin(&config, McpTransportType::Http, Some(base))
            .unwrap()
            .unwrap();
        let mut missing_override = config.clone();
        missing_override["connector_plugin"]
            .as_object_mut()
            .unwrap()
            .remove("base_url_override");
        assert!(
            mcp_plugin_config::compile_plugin(
                &missing_override,
                McpTransportType::Http,
                Some(base)
            )
            .is_err()
        );
        if id == "odoo" {
            for configuration in [
                json!({}),
                json!({"database":"demo\r\nHost: attacker"}),
                json!({"database":"demo","arbitrary":"secret"}),
            ] {
                let mut invalid = config.clone();
                invalid["connector_plugin"]["configuration"] = configuration;
                assert!(
                    mcp_plugin_config::compile_plugin(&invalid, McpTransportType::Http, Some(base))
                        .is_err()
                );
            }
        }
    }
}

#[tokio::test]
async fn three_hundred_real_mcp_users_share_a_small_idle_cache_without_token_mixups() {
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    use mcp_client::McpServerClient;
    use models::mcp_servers::McpTransportType;
    use serde_json::{Value, json};
    use std::sync::Arc;
    let router = Router::new().route("/mcp", post(|headers:HeaderMap, Json(message):Json<Value>| async move {
        if message.get("id").is_none() { return StatusCode::ACCEPTED.into_response(); }
        let result = match message["method"].as_str().unwrap() {
            "initialize" => json!({"protocolVersion":message["params"]["protocolVersion"],"capabilities":{"tools":{}},"serverInfo":{"name":"cache-fixture","version":"1"}}),
            "tools/call" => {
                let owner = message["params"]["arguments"]["owner"].as_str().unwrap();
                assert_eq!(headers.get("authorization").unwrap(), format!("Bearer {owner}").as_str());
                json!({"content":[{"type":"text","text":owner}],"structuredContent":{"owner":owner}})
            }
            "tools/list" => json!({"tools":[]}),
            _ => return StatusCode::BAD_REQUEST.into_response(),
        };
        Json(json!({"jsonrpc":"2.0","id":message["id"],"result":result})).into_response()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = Arc::new(McpServerClient::new(
        uuid::Uuid::new_v4(),
        "cache-fixture".into(),
        McpTransportType::Http,
        Some(url),
        json!({"transport_config":{"session_cache":{"max_entries":4,"max_in_flight":8,"wait_timeout_seconds":30}}}),
        None,
    ));
    let mut tasks = tokio::task::JoinSet::new();
    for number in 0..300 {
        let client = client.clone();
        tasks.spawn(async move {
            let owner = format!("fixture-user-{number}");
            let result = client
                .call_tool(
                    "ping",
                    json!({"owner":owner}),
                    Some(owner.clone()),
                    Some(uuid::Uuid::new_v4()),
                )
                .await
                .unwrap();
            assert_eq!(result.structured_content.unwrap()["owner"], owner);
        });
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }
    })
    .await;
    tasks.abort_all();
    server.abort();
    result.unwrap();
}

#[tokio::test]
#[ignore = "requires local Navikt server and GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR"]
async fn navikt_exchanges_and_refreshes_every_oauth_catalog_setup_with_production_code() {
    use axum::{
        Json, Router,
        body::Bytes,
        extract::{OriginalUri, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
    };
    use mcp_client::{
        build_authorization_url, exchange_code, oauth_config_from_connection, refresh_token,
    };
    use serde_json::{Value, json};
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

    struct Observed {
        provider: String,
        form: HashMap<String, String>,
        in_query: bool,
    }
    #[derive(Clone)]
    struct Adapter {
        base: String,
        http: reqwest::Client,
        seen: Arc<tokio::sync::Mutex<Vec<Observed>>>,
    }
    let base = std::env::var("MOCK_OAUTH2_BASE_URL").expect("local Navikt URL");
    assert_eq!(
        url::Url::parse(&base).unwrap().host_str(),
        Some("127.0.0.1")
    );
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let adapter = Adapter {
        base: base.clone(),
        http: http.clone(),
        seen: Default::default(),
    };
    let router = Router::new().fallback(|State(adapter):State<Adapter>, OriginalUri(uri):OriginalUri, headers:HeaderMap, body:Bytes| async move {
        let segments: Vec<_> = uri.path().trim_matches('/').split('/').collect();
        let id = segments[0];
        let is_refresh = segments.get(1) == Some(&"refresh");
        let is_json = headers.get("content-type").is_some_and(|value| value.to_str().unwrap().contains("application/json"));
        let input = if body.is_empty() { uri.query().unwrap_or("").as_bytes() } else { &body };
        let mut form:HashMap<String,String> = if is_json { serde_json::from_slice(&body).unwrap() } else { url::form_urlencoded::parse(input).into_owned().collect() };
        adapter.seen.lock().await.push(Observed {provider:id.into(),form:form.clone(),in_query:uri.query().is_some()});
        if is_refresh && !form.contains_key("grant_type") { form.insert("grant_type".into(), "refresh_token".into()); }
        let mut request = adapter.http.post(format!("{}/{id}/token",adapter.base)).form(&form);
        if let Some(authorization)=headers.get("authorization") { request=request.header("authorization", authorization); }
        let response = request.send().await.unwrap();
        let status = response.status();
        let mut value:Value = response.json().await.unwrap();
        if id=="slack" && status.is_success() {
            if !is_refresh { value=json!({"ok":true,"access_token":"bot-must-not-be-selected","authed_user":value}); }
            else { value["ok"]=json!(true); }
        }
        (StatusCode::from_u16(status.as_u16()).unwrap(),Json(value)).into_response()
    }).with_state(adapter.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let catalog = PathBuf::from(std::env::var("GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR").unwrap());
    let listing: Vec<Value> = serde_json::from_slice(
        &std::fs::read(catalog.parent().unwrap().join("connector-plugins.json")).unwrap(),
    )
    .unwrap();
    let mut tested = 0;
    for entry in listing {
        let id = entry["key"].as_str().unwrap();
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(catalog.join(id).join("plugin.json")).unwrap())
                .unwrap();
        let setup: Value = serde_json::from_slice(
            &std::fs::read(
                catalog
                    .join(id)
                    .join("versions")
                    .join(manifest["version"].as_str().unwrap())
                    .join("setup.json"),
            )
            .unwrap(),
        )
        .unwrap();
        if setup["authentication"]["kind"] != "oauth2" {
            continue;
        }
        let mut oauth = setup["authentication"]["oauth"].clone();
        oauth["auth_url"] = json!(format!("{base}/{id}/authorize"));
        oauth["token_url"] = json!(format!("{origin}/{id}/token"));
        oauth["redirect_url"] = json!("http://localhost:5173/mcp/oauth/callback");
        // Mock grants use OIDC/offline_access; real vendor scopes/extras are verified separately.
        oauth["scopes"] = json!(["openid", "offline_access"]);
        oauth["scope_separator"] = json!("space");
        oauth["extra_params"] = json!({});
        if oauth.get("refresh_token_url").is_some() {
            oauth["refresh_token_url"] = json!(format!("{origin}/{id}/refresh"));
        }
        let config = oauth_config_from_connection(
            &json!({"oauth":oauth}),
            &format!("fixture-{id}"),
            Some("fixture-secret".into()),
        )
        .unwrap();
        let authorization = build_authorization_url(&config).unwrap();
        let response = http
            .get(&authorization.authorization_url)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::FOUND,
            "{id}: authorize"
        );
        let callback = url::Url::parse(
            response
                .headers()
                .get("location")
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            callback.origin().ascii_serialization(),
            "http://localhost:5173"
        );
        let params: HashMap<_, _> = callback.query_pairs().into_owned().collect();
        assert_eq!(
            params["state"], authorization.state,
            "{id}: state round trip"
        );
        let tokens = exchange_code(
            &config,
            &params["code"],
            &authorization.pkce_verifier,
            &http,
        )
        .await
        .unwrap_or_else(|error| panic!("{id}: {error}"));
        assert!(!tokens.access_token.is_empty());
        assert_ne!(tokens.access_token, "bot-must-not-be-selected");
        let refreshed = refresh_token(
            &config,
            tokens.refresh_token.as_ref().expect("mock offline token"),
            &http,
        )
        .await
        .unwrap_or_else(|error| panic!("{id}: refresh {error}"));
        assert!(!refreshed.access_token.is_empty());
        if id == "monday" {
            assert!(refreshed.expires_at.is_some());
        }
        if id == "calendly" {
            let seen = adapter.seen.lock().await;
            for request in seen.iter().filter(|request| request.provider == id) {
                assert_eq!(request.form["client_id"], "fixture-calendly");
                assert!(!request.form.contains_key("client_secret"));
            }
        }
        if id == "figma" {
            let seen = adapter.seen.lock().await;
            let request = seen
                .iter()
                .rev()
                .find(|request| request.provider == id)
                .unwrap();
            assert_eq!(request.form.len(), 1);
            assert!(request.form.contains_key("refresh_token"));
        }
        if id == "miro" {
            let seen = adapter.seen.lock().await;
            let requests: Vec<_> = seen
                .iter()
                .filter(|request| request.provider == id)
                .collect();
            assert!(requests[0].in_query);
            assert!(!requests[1].in_query);
        }
        tested += 1;
        println!("PASS {id}: production code exchange, token parsing, refresh");
    }
    server.abort();
    assert!(tested > 0, "OAuth catalog must not be empty");
    println!("PASS all {tested} OAuth catalog setups");
}
