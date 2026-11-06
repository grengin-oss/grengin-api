// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::Request,
    http::{Response, StatusCode, header},
};
use connector_server_plugin::{
    CallContext, CompiledConnector, ConnectorError, ConnectorPluginConfig,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::Mutex};

const TOKEN: &str = "private-token-canary";

/// Every request the mock saw: raw path with query, and its Authorization header.
type Seen = Arc<Mutex<Vec<(String, Option<String>)>>>;

async fn serve() -> (String, Seen) {
    let seen: Seen = Arc::default();
    let log = seen.clone();
    let router = Router::new().fallback(move |request: Request| {
        let log = log.clone();
        async move {
            let target = request
                .uri()
                .path_and_query()
                .map(|value| value.to_string())
                .unwrap_or_default();
            let auth = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            log.lock().await.push((target.clone(), auth));
            let redirect = |location: &str| {
                Response::builder()
                    .status(StatusCode::FOUND)
                    .header(header::LOCATION, location)
                    .body(Body::empty())
                    .unwrap()
            };
            match request.uri().path() {
                "/api/start" => redirect("/download/file.txt?sig=signed"),
                "/api/offsite" => redirect("http://files.example.net/file.txt"),
                "/api/loop" => redirect("/api/loop"),
                "/api/octet" => Response::builder()
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from("plain download"))
                    .unwrap(),
                "/api/binary" => Response::builder()
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(&b"PK\x03\x04\x00\x00zip data"[..]))
                    .unwrap(),
                "/download/file.txt" => Response::builder()
                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .body(Body::from("downloaded text"))
                    .unwrap(),
                _ => Response::builder()
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"path": target}).to_string()))
                    .unwrap(),
            }
        }
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, seen)
}

fn tool(name: &str, path: &str, extra: Value) -> Value {
    let mut tool = json!({
        "name": name, "description": "Routing test tool", "read_only": true,
        "input_schema": {"type": "object", "properties": {"value": {"type": "string"}}},
        "request": {"method": "GET", "path": path}
    });
    merge(&mut tool, extra);
    tool
}

fn merge(target: &mut Value, extra: Value) {
    match (target, extra) {
        (Value::Object(target), Value::Object(extra)) => {
            for (key, value) in extra {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, extra) => *target = extra,
    }
}

fn compile(
    origin: &str,
    tools: Vec<Value>,
    extra: Value,
) -> Result<CompiledConnector, ConnectorError> {
    let mut manifest = json!({
        "schema_version": 1, "id": "routing-test", "name": "Routing test", "version": "1.0.0",
        "base_url": format!("{origin}/api"), "auth": "oauth2", "tools": tools
    });
    merge(&mut manifest, extra);
    CompiledConnector::compile(ConnectorPluginConfig {
        manifest,
        allow_insecure_http: true,
        allow_private_network: true,
        ..Default::default()
    })
}

async fn call(
    plugin: &CompiledConnector,
    name: &str,
    args: Value,
) -> Result<Value, ConnectorError> {
    plugin
        .call(
            name,
            args,
            CallContext {
                user_id: "alice",
                access_token: Some(TOKEN),
            },
        )
        .await
}

fn download(path: &str, redirects: Value) -> Value {
    tool(
        "download",
        path,
        json!({"response": {
            "format": "text", "overflow": "truncate", "max_result_bytes": 4096, "redirects": redirects
        }}),
    )
}

#[tokio::test]
async fn redirects_reach_allowlisted_downloads_without_forwarding_credentials() {
    let (origin, seen) = serve().await;
    let policy = json!({"allowed_hosts": ["127.0.0.1"], "max_hops": 2});
    let plugin = compile(&origin, vec![download("/start", policy)], json!({})).unwrap();
    let result = call(&plugin, "download", json!({})).await.unwrap();
    assert_eq!(result["content"], "downloaded text");
    let seen = seen.lock().await.clone();
    assert_eq!(
        seen[0],
        ("/api/start".into(), Some(format!("Bearer {TOKEN}")))
    );
    assert_eq!(seen[1], ("/download/file.txt?sig=signed".into(), None));
}

#[tokio::test]
async fn redirects_to_other_hosts_loops_or_tools_without_a_policy_fail_closed() {
    let (origin, seen) = serve().await;
    let policy = json!({"allowed_hosts": ["127.0.0.1"], "max_hops": 2});
    let plugin = compile(
        &origin,
        vec![
            download("/offsite", policy.clone()),
            tool(
                "looping",
                "/loop",
                json!({"response": {"redirects": policy}}),
            ),
            tool("strict", "/start", json!({})),
        ],
        json!({}),
    )
    .unwrap();
    assert!(matches!(
        call(&plugin, "download", json!({})).await,
        Err(ConnectorError::Destination)
    ));
    assert!(matches!(
        call(&plugin, "looping", json!({})).await,
        Err(ConnectorError::Destination)
    ));
    assert!(matches!(
        call(&plugin, "strict", json!({})).await,
        Err(ConnectorError::Http { status: 302, .. })
    ));
    let seen = seen.lock().await;
    assert!(
        seen.iter()
            .all(|(path, _)| !path.contains("files.example.net"))
    );
    assert_eq!(
        seen.iter().filter(|(path, _)| path == "/api/loop").count(),
        3
    );
}

#[tokio::test]
async fn redirect_policies_are_limited_to_bounded_get_requests() {
    let (origin, _) = serve().await;
    for (method, policy) in [
        (
            "POST",
            json!({"allowed_hosts": ["127.0.0.1"], "max_hops": 1}),
        ),
        (
            "GET",
            json!({"allowed_hosts": ["127.0.0.1"], "max_hops": 0}),
        ),
        (
            "GET",
            json!({"allowed_hosts": ["127.0.0.1"], "max_hops": 4}),
        ),
        ("GET", json!({"allowed_hosts": [], "max_hops": 1})),
        ("GET", json!({"allowed_hosts": [".com"], "max_hops": 1})),
        (
            "GET",
            json!({"allowed_hosts": ["Files.Example.com"], "max_hops": 1}),
        ),
        (
            "GET",
            json!({"allowed_hosts": ["files.example.com/path"], "max_hops": 1}),
        ),
    ] {
        let tool = tool(
            "t",
            "/x",
            json!({"request": {"method": method}, "response": {"redirects": policy}}),
        );
        assert!(
            compile(&origin, vec![tool], json!({})).is_err(),
            "{method} {policy}"
        );
    }
}

#[tokio::test]
async fn embedded_placeholders_quote_odata_literals_and_reject_breakouts() {
    let (origin, seen) = serve().await;
    let value = json!({"$get": "request.value"});
    let plugin = compile(
        &origin,
        vec![
            tool(
                "quoted",
                "/A_BusinessPartner('{value}')",
                json!({"request": {"path_parameters": {"value": value}}}),
            ),
            tool(
                "plain",
                "/items({value})",
                json!({"request": {"path_parameters": {"value": value}}}),
            ),
        ],
        json!({}),
    )
    .unwrap();
    call(&plugin, "quoted", json!({"value": "O'Brien Ltd"}))
        .await
        .unwrap();
    assert_eq!(
        seen.lock().await[0].0,
        "/api/A_BusinessPartner('O''Brien%20Ltd')"
    );
    call(&plugin, "plain", json!({"value": "42"}))
        .await
        .unwrap();
    assert_eq!(seen.lock().await[1].0, "/api/items(42)");
    for (name, value) in [
        ("quoted", "a/b"),
        ("quoted", "a%2Fb"),
        ("quoted", ""),
        ("plain", "1)"),
        ("plain", "1,2"),
        ("plain", "x'y"),
    ] {
        assert!(
            matches!(
                call(&plugin, name, json!({"value": value})).await,
                Err(ConnectorError::Arguments)
            ),
            "{name} {value}"
        );
    }
    assert_eq!(seen.lock().await.len(), 2);
    for path in [
        "/x:{value}",
        "/a{value}{value}",
        "/a{value}}",
        "/a{value}b c",
    ] {
        let tool = tool(
            "t",
            path,
            json!({"request": {"path_parameters": {"value": value}}}),
        );
        assert!(compile(&origin, vec![tool], json!({})).is_err(), "{path}");
    }
}

#[tokio::test]
async fn opaque_path_parameters_encode_reserved_characters_once_or_twice() {
    let (origin, seen) = serve().await;
    let parameters = json!({"path_parameters": {"value": {"$get": "request.value"}}});
    let mut once = parameters.clone();
    merge(
        &mut once,
        json!({"path_parameter_encoding": {"value": "opaque"}}),
    );
    let mut zoom = parameters.clone();
    merge(
        &mut zoom,
        json!({"path_parameter_encoding": {"value": "opaque_double_if_slash"}}),
    );
    let plugin = compile(
        &origin,
        vec![
            tool(
                "once",
                "/meetings/{value}/summary",
                json!({"request": once}),
            ),
            tool(
                "zoom",
                "/meetings/{value}/summary",
                json!({"request": zoom}),
            ),
        ],
        json!({}),
    )
    .unwrap();
    for (name, value, expected) in [
        ("once", "abc/d+==", "/api/meetings/abc%2Fd+==/summary"),
        (
            "zoom",
            "abc/d+==",
            "/api/meetings/abc%252Fd%252B%253D%253D/summary",
        ),
        (
            "zoom",
            "/abc//d==",
            "/api/meetings/%252Fabc%252F%252Fd%253D%253D/summary",
        ),
        (
            "zoom",
            "aDYlohsHRtCd4ii1uC2+hA==",
            "/api/meetings/aDYlohsHRtCd4ii1uC2+hA==/summary",
        ),
    ] {
        call(&plugin, name, json!({"value": value})).await.unwrap();
        assert_eq!(
            seen.lock().await.last().unwrap().0,
            expected,
            "{name} {value}"
        );
    }
    for value in ["", ".", "..", "a/../b", "a\nb"] {
        assert!(
            matches!(
                call(&plugin, "zoom", json!({"value": value})).await,
                Err(ConnectorError::Arguments)
            ),
            "{value:?}"
        );
    }
    let embedded = tool(
        "t",
        "/m('{value}')",
        json!({"request": {"path_parameters": {"value": {"$get": "request.value"}},
                           "path_parameter_encoding": {"value": "opaque"}}}),
    );
    assert!(compile(&origin, vec![embedded], json!({})).is_err());
}

#[tokio::test]
async fn tools_send_credentials_only_to_declared_additional_hosts() {
    let (origin, seen) = serve().await;
    let content = json!({"additional_base_urls": {"content": format!("{origin}/content")}});
    let plugin = compile(
        &origin,
        vec![tool(
            "download",
            "/files/download",
            json!({"request": {"base": "content"}}),
        )],
        content.clone(),
    )
    .unwrap();
    call(&plugin, "download", json!({})).await.unwrap();
    assert_eq!(
        seen.lock().await[0],
        (
            "/content/files/download".into(),
            Some(format!("Bearer {TOKEN}"))
        )
    );
    let paged = tool(
        "t",
        "/x",
        json!({"request": {"base": "content"}, "response": {
        "pagination": {"items_pointer": "/items", "next_pointer": "/next", "max_pages": 2}}}),
    );
    assert!(compile(&origin, vec![paged], content).is_err());
    let unknown = tool("t", "/x", json!({"request": {"base": "missing"}}));
    assert!(compile(&origin, vec![unknown.clone()], json!({})).is_err());
    for urls in [
        json!({"content": "ftp://files.example.com/"}),
        json!({"content": format!("{origin}/content?token=x")}),
        json!({"Bad-Name": format!("{origin}/content")}),
        json!({"a": origin, "b": origin, "c": origin, "d": origin, "e": origin}),
    ] {
        let plain = tool("t", "/x", json!({}));
        assert!(
            compile(&origin, vec![plain], json!({"additional_base_urls": urls})).is_err(),
            "{urls}"
        );
    }
}

#[tokio::test]
async fn untyped_downloads_are_read_as_text_only_when_opted_in_and_not_binary() {
    let (origin, _) = serve().await;
    let text = |name: &str, path: &str, accept: bool| {
        tool(
            name,
            path,
            json!({"response": {
                "format": "text", "overflow": "truncate", "max_result_bytes": 4096,
                "accept_octet_stream": accept
            }}),
        )
    };
    let plugin = compile(
        &origin,
        vec![
            text("opted", "/octet", true),
            text("binary", "/binary", true),
            text("strict", "/octet", false),
        ],
        json!({}),
    )
    .unwrap();
    assert_eq!(
        call(&plugin, "opted", json!({})).await.unwrap()["content"],
        "plain download"
    );
    assert!(matches!(
        call(&plugin, "binary", json!({})).await,
        Err(ConnectorError::Response)
    ));
    assert!(matches!(
        call(&plugin, "strict", json!({})).await,
        Err(ConnectorError::Response)
    ));
    let json_tool = tool(
        "t",
        "/octet",
        json!({"response": {"accept_octet_stream": true}}),
    );
    assert!(compile(&origin, vec![json_tool], json!({})).is_err());
}
