// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use connector_server_plugin::{
    CallContext, CompiledConnector, ConnectorError, ConnectorPluginConfig,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, task::JoinHandle};

struct Server {
    base: String,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(router: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server { base, task }
}

fn manifest(base: &str) -> Value {
    json!({
        "schema_version":1, "id":"test", "name":"Test", "version":"1.0.0",
        "base_url":base, "auth":"oauth2",
        "tools":[{
            "name":"lookup", "description":"Look up a record", "read_only":true,
            "input_schema":{"type":"object", "properties":{"id":{"type":"string", "minLength":1}}, "required":["id"], "additionalProperties":false},
            "request":{"method":"GET", "path":"/records/{id}", "path_parameters":{"id":{"$get":"request.id"}}}
        }]
    })
}

fn config(value: Value) -> ConnectorPluginConfig {
    ConnectorPluginConfig {
        manifest: value,
        auth_mode: None,
        base_url_override: None,
        configuration: json!({}),
        allow_insecure_http: true,
        allow_private_network: true,
    }
}

fn context() -> CallContext<'static> {
    CallContext {
        user_id: "alice",
        access_token: Some("alice-token"),
    }
}

#[test]
fn explicit_token_mode_cannot_remove_authentication_or_invent_oauth() {
    use connector_server_plugin::ConnectorAuth;
    for (primary, mode, allowed) in [
        ("oauth2", ConnectorAuth::ApiKey, true),
        ("oauth2", ConnectorAuth::None, false),
        ("api_key", ConnectorAuth::Oauth2, false),
        ("api_key", ConnectorAuth::None, false),
        ("none", ConnectorAuth::ApiKey, false),
        ("none", ConnectorAuth::Oauth2, false),
    ] {
        let mut value = manifest("https://example.com/v1");
        value["auth"] = json!(primary);
        let mut cfg = config(value);
        cfg.auth_mode = Some(mode);
        let result = CompiledConnector::compile(cfg);
        assert_eq!(result.is_ok(), allowed, "{primary} {mode:?}");
        if let Ok(plugin) = result {
            assert_eq!(plugin.manifest().auth, mode);
        }
    }
}

#[test]
fn optional_presentation_is_bounded_and_does_not_change_tool_behavior() {
    let mut value = manifest("https://example.com/v1");
    value["presentation"] = json!({"suggested_prompts":["Find the project roadmap."]});
    let plugin = CompiledConnector::compile(config(value.clone())).unwrap();
    assert_eq!(plugin.manifest().tools.len(), 1);
    assert_eq!(
        plugin
            .manifest()
            .presentation
            .as_ref()
            .unwrap()
            .suggested_prompts[0],
        "Find the project roadmap."
    );
    for invalid in [
        json!([""]),
        json!(["x".repeat(241)]),
        json!(["x\ny"]),
        json!(vec!["example"; 9]),
    ] {
        value["presentation"]["suggested_prompts"] = invalid;
        assert!(CompiledConnector::compile(config(value.clone())).is_err());
    }
}

#[test]
fn tenant_configuration_requires_explicit_valid_values_and_safe_destinations() {
    let mut value = manifest("https://login.salesforce.com/v65");
    value["configuration_schema"] = json!({"type":"object","properties":{"tenant":{"type":"string","minLength":1}},"required":["tenant"],"additionalProperties":false});
    value["base_url_policy"] =
        json!({"allowed_host_suffixes":[".salesforce.com"],"required_override":true});
    assert!(
        CompiledConnector::compile(config(value.clone()))
            .unwrap()
            .validate_configuration()
            .is_err()
    );
    let mut setup = config(value.clone());
    setup.configuration = json!({"tenant":"test"});
    setup.base_url_override = Some("https://example.salesforce.com/v65".into());
    assert!(
        CompiledConnector::compile(setup)
            .unwrap()
            .validate_configuration()
            .is_ok()
    );
    for url in [
        "https://salesforce.com.attacker.example",
        "https://attacker.example",
        "http://169.254.169.254",
    ] {
        let mut setup = config(value.clone());
        setup.base_url_override = Some(url.into());
        assert!(CompiledConnector::compile(setup).is_err());
    }
    value["configuration_schema"]["properties"]["tenant"] =
        json!({"$ref":"https://attacker.example/schema"});
    assert!(CompiledConnector::compile(config(value)).is_err());
}

#[test]
fn self_hosted_public_hosts_need_explicit_policy_and_override() {
    let mut value = manifest("https://configure.invalid/json/2");
    value["base_url_policy"] = json!({
        "allowed_host_suffixes": [], "required_override": true, "allow_custom_public_host": true
    });
    let mut setup = config(value.clone());
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
        assert!(CompiledConnector::compile(config(value.clone())).is_err());
    }
}

#[test]
fn credential_delivery_cannot_override_routing_or_collide_with_mapped_headers() {
    for name in ["Host", "Cookie", "X-Forwarded-Host", "Bad\nHeader"] {
        let mut value = manifest("https://example.com");
        value["credential_delivery"] = json!({"location":"header","name":name});
        assert!(CompiledConnector::compile(config(value)).is_err());
    }
    let mut value = manifest("https://example.com");
    value["credential_delivery"] = json!({"location":"header","name":"X-Api-Key"});
    value["tools"][0]["request"]["headers"] = json!({"x-api-key":"forged"});
    assert!(CompiledConnector::compile(config(value)).is_err());
    let mut value = manifest("https://example.com");
    value["credential_delivery"] = json!({"location":"json_body","field":"key"});
    assert!(CompiledConnector::compile(config(value)).is_err());
}

#[tokio::test]
async fn json_body_credentials_are_private_and_cannot_be_supplied_by_tool_arguments() {
    let server = serve(Router::new().fallback(
        |headers: HeaderMap, Json(body): Json<Value>| async move {
            assert!(!headers.contains_key("authorization"));
            Json(body)
        },
    ))
    .await;
    let mut value = manifest(&server.base);
    value["auth"] = json!("api_key");
    value["credential_delivery"] = json!({"location":"json_body","field":"key"});
    value["tools"][0]["request"] =
        json!({"method":"POST","path":"/read","body":{"id":{"$get":"request.id"}}});
    let plugin = CompiledConnector::compile(config(value.clone())).unwrap();
    let result = plugin
        .call("lookup", json!({"id":"one"}), context())
        .await
        .unwrap();
    assert_eq!(result, json!({"id":"one","key":"alice-token"}));
    for token in ["", " ", "a\nb", "a b"] {
        assert!(matches!(
            plugin
                .call(
                    "lookup",
                    json!({"id":"one"}),
                    CallContext {
                        user_id: "alice",
                        access_token: Some(token)
                    }
                )
                .await,
            Err(ConnectorError::Authentication)
        ));
    }
    value["tools"][0]["request"]["body"]["key"] = json!({"$get":"request.id"});
    let collision = CompiledConnector::compile(config(value.clone())).unwrap();
    assert!(matches!(
        collision
            .call("lookup", json!({"id":"fake-key"}), context())
            .await,
        Err(ConnectorError::Manifest(_))
    ));
    value["tools"][0]["request"]["body"] = json!({"stolen":{"$get":"session.access_token"}});
    assert!(matches!(
        CompiledConnector::compile(config(value))
            .unwrap()
            .call("lookup", json!({"id":"one"}), context())
            .await,
        Err(ConnectorError::Mapping)
    ));
}

#[tokio::test]
async fn graphql_errors_fail_even_with_http_200_and_malformed_errors_are_rejected() {
    for errors in [
        json!([{"message":"sensitive-vendor-error"}]),
        json!("malformed"),
    ] {
        let server = serve(Router::new().fallback(move || {
            let errors = errors.clone();
            async move { Json(json!({"errors":errors})) }
        }))
        .await;
        let mut value = manifest(&server.base);
        value["tools"][0]["response"] = json!({"error_array_pointer":"/errors"});
        let error = CompiledConnector::compile(config(value))
            .unwrap()
            .call("lookup", json!({"id":"one"}), context())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ConnectorError::ProviderRejected | ConnectorError::Response
        ));
        assert!(!error.to_string().contains("sensitive-vendor-error"));
    }
}

#[tokio::test]
async fn rejects_oversized_urls_headers_and_mapping_expansion_before_network() {
    for target in ["query", "headers"] {
        let mut value = manifest("http://127.0.0.1:1/v1");
        value["tools"][0]["request"][target] = json!({"X-Large":"x".repeat(17000)});
        let plugin = CompiledConnector::compile(config(value)).unwrap();
        assert!(matches!(
            plugin.call("lookup", json!({"id":"one"}), context()).await,
            Err(ConnectorError::Arguments)
        ));
    }
    let mut value = manifest("http://127.0.0.1:1/v1");
    value["tools"][0]["input_schema"] =
        json!({"type":"object","properties":{"items":{"type":"array"}},"required":["items"]});
    value["tools"][0]["request"] =
        json!({"method":"POST","path":"/records","body":{"$map":"request.items","using":"expand"}});
    value["mappings"] =
        json!({"expand":{"$map":"request.items","using":"copy"},"copy":{"$get":"item"}});
    let plugin = CompiledConnector::compile(config(value)).unwrap();
    assert!(matches!(
        plugin
            .call("lookup", json!({"items":vec![0;1000]}), context())
            .await,
        Err(ConnectorError::Mapping)
    ));
}

#[test]
fn reference_manifest_compiles() {
    let value: Value = serde_json::from_str(include_str!("../example.json")).unwrap();
    let plugin = CompiledConnector::compile(ConnectorPluginConfig {
        manifest: value,
        ..Default::default()
    })
    .unwrap();
    assert_eq!(plugin.manifest().tools.len(), 4);
}

#[test]
fn generated_schema_is_current_and_accepts_the_reference_manifest() {
    let schema: Value =
        serde_json::from_str(include_str!("../schema/connector-plugin-v1.schema.json")).unwrap();
    assert_eq!(
        schema,
        serde_json::to_value(schemars::schema_for!(
            connector_server_plugin::ConnectorManifest
        ))
        .unwrap()
    );
    let manifest: Value = serde_json::from_str(include_str!("../example.json")).unwrap();
    assert!(jsonschema::is_valid(&schema, &manifest));
}

#[test]
fn rejects_invalid_manifest_and_unsupported_versions() {
    for (pointer, replacement) in [
        ("/schema_version", json!(2)),
        ("/version", json!("latest")),
        ("/id", json!("../id")),
        ("/tools/0/name", json!("bad.name")),
        ("/tools/0/input_schema/type", json!("array")),
        ("/tools/0/request/method", json!("CONNECT")),
        ("/auth", json!("global_env")),
        ("/tools/0/description", json!("")),
    ] {
        let mut value = manifest("https://example.com/v1");
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            CompiledConnector::compile(config(value)).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn duplicate_tools_and_unknown_fields_are_rejected() {
    let mut value = manifest("https://example.com/v1");
    let tool = value["tools"][0].clone();
    value["tools"].as_array_mut().unwrap().push(tool);
    assert!(CompiledConnector::compile(config(value)).is_err());
    let mut value = manifest("https://example.com/v1");
    value["command"] = json!("sh");
    assert!(CompiledConnector::compile(config(value)).is_err());
}

#[test]
fn external_schema_references_are_rejected() {
    for reference in [
        "https://evil.example/schema",
        "file:///etc/passwd",
        "urn:external:schema",
    ] {
        let mut value = manifest("https://example.com/v1");
        value["tools"][0]["input_schema"]["properties"]["id"] = json!({"$ref":reference});
        assert!(CompiledConnector::compile(config(value)).is_err());
    }
}

#[test]
fn headers_cannot_override_credentials_or_routing() {
    for header in [
        "Authorization",
        "authorization",
        "Host",
        "Cookie",
        "X-Forwarded-Host",
        "Content-Length",
        "Bad\nName",
    ] {
        let mut value = manifest("https://example.com/v1");
        value["tools"][0]["request"]["headers"] = json!({header:"secret"});
        assert!(
            CompiledConnector::compile(config(value)).is_err(),
            "{header}"
        );
    }
}

#[test]
fn rejects_traversal_authorities_and_malformed_path_templates() {
    for path in [
        "../escape",
        "/../escape",
        "//evil.example/path",
        "https://evil.example/path",
        "/%2e%2e/escape",
        "/records/{unknown}",
        "/records/{id}{id}",
        "/records/{id",
        "/records/id}",
        "/records/{id}:x",
        "/records/{id} x",
        "/records\\id",
        "/records?token=secret",
        "/https://evil.example/path",
        "/records::query",
        "/:query",
        "/records:",
    ] {
        let mut value = manifest("https://example.com/v1");
        value["tools"][0]["request"]["path"] = json!(path);
        assert!(CompiledConnector::compile(config(value)).is_err(), "{path}");
    }
}

#[test]
fn a_placeholder_may_sit_among_safe_literal_text_in_one_segment() {
    for path in ["/records/{id}.json", "/Records('{id}')"] {
        let mut value = manifest("https://example.com/v1");
        value["tools"][0]["request"]["path"] = json!(path);
        assert!(CompiledConnector::compile(config(value)).is_ok(), "{path}");
    }
}

#[test]
fn static_resource_actions_cannot_change_the_url_authority() {
    let mut value = manifest("https://example.com/v1");
    value["tools"][0]["request"]["path"] = json!("/records:search");
    value["tools"][0]["request"]["path_parameters"] = json!({});
    assert!(CompiledConnector::compile(config(value)).is_ok());
}

#[test]
fn local_network_requires_explicit_approval_and_metadata_is_always_blocked() {
    for base in [
        "http://127.0.0.1:1234",
        "https://127.0.0.1",
        "https://[::1]",
        "https://10.0.0.1",
        "https://localhost",
    ] {
        let cfg = ConnectorPluginConfig {
            manifest: manifest(base),
            ..Default::default()
        };
        assert!(CompiledConnector::compile(cfg).is_err(), "{base}");
    }
    for base in [
        "http://169.254.169.254",
        "http://169.254.170.2",
        "http://[fd00:ec2::254]",
        "http://metadata.google.internal",
        "http://[::ffff:169.254.169.254]",
    ] {
        assert!(
            CompiledConnector::compile(config(manifest(base))).is_err(),
            "{base}"
        );
    }
}

#[test]
fn contradictory_annotations_and_unbounded_pagination_are_rejected() {
    for method in ["PATCH", "PUT", "DELETE"] {
        let mut value = manifest("https://example.com");
        value["tools"][0]["request"]["method"] = json!(method);
        assert!(CompiledConnector::compile(config(value)).is_err());
    }
    for pages in [0, 11, 65535] {
        let mut value = manifest("https://example.com");
        value["tools"][0]["response"] = json!({"pagination":{"items_pointer":"/value", "next_pointer":"/next", "max_pages":pages}});
        assert!(CompiledConnector::compile(config(value)).is_err());
    }
}

#[tokio::test]
async fn rejects_invalid_args_missing_auth_unknown_tools_before_network() {
    let plugin = CompiledConnector::compile(config(manifest("http://127.0.0.1:1"))).unwrap();
    for args in [
        json!({}),
        json!({"id":12}),
        json!({"id":"a", "other":true}),
        json!([]),
    ] {
        assert!(matches!(
            plugin.call("lookup", args, context()).await,
            Err(ConnectorError::Arguments)
        ));
    }
    for token in [None, Some(""), Some(" \t"), Some("bad\r\nheader")] {
        assert!(matches!(
            plugin
                .call(
                    "lookup",
                    json!({"id":"a"}),
                    CallContext {
                        user_id: "a",
                        access_token: token
                    }
                )
                .await,
            Err(ConnectorError::Authentication)
        ));
    }
    assert!(matches!(
        plugin
            .call(
                "lookup",
                json!({"id":"a"}),
                CallContext {
                    user_id: "",
                    access_token: Some("token")
                }
            )
            .await,
        Err(ConnectorError::Authentication)
    ));
    assert!(matches!(
        plugin.call("other", json!({"id":"a"}), context()).await,
        Err(ConnectorError::ToolNotFound)
    ));
}

#[tokio::test]
async fn preserves_base_path_and_does_not_share_user_tokens() {
    let server = serve(Router::new().route("/v1/records/{id}", get(|Path(id):Path<String>, headers:HeaderMap| async move {
        Json(json!({"id":id, "authorization":headers.get("authorization").unwrap().to_str().unwrap()}))
    }))).await;
    let plugin =
        CompiledConnector::compile(config(manifest(&format!("{}/v1", server.base)))).unwrap();
    let alice = plugin.call("lookup", json!({"id":"a"}), context());
    let bob = plugin.call(
        "lookup",
        json!({"id":"b"}),
        CallContext {
            user_id: "bob",
            access_token: Some("bob-token"),
        },
    );
    let (alice, bob) = tokio::join!(alice, bob);
    assert_eq!(
        alice.unwrap(),
        json!({"id":"a", "authorization":"Bearer alice-token"})
    );
    assert_eq!(
        bob.unwrap(),
        json!({"id":"b", "authorization":"Bearer bob-token"})
    );
    assert_eq!(
        plugin
            .call("lookup", json!({"id":"c"}), context())
            .await
            .unwrap()["authorization"],
        "Bearer alice-token"
    );
}

#[tokio::test]
async fn request_body_query_headers_and_response_use_existing_mapping_dsl() {
    let server = serve(Router::new().route("/v1/records/{id}", post(|Path(id):Path<String>, Query(query):Query<std::collections::HashMap<String,String>>, headers:HeaderMap, Json(body):Json<Value>| async move {
        Json(json!({"value":{"id":id, "query":query, "body":body, "header":headers["x-feature"].to_str().unwrap()}}))
    }))).await;
    let mut value = manifest(&format!("{}/v1", server.base));
    let tool = &mut value["tools"][0];
    tool["read_only"] = json!(false);
    tool["request"]["method"] = json!("POST");
    tool["request"]["body"] = json!({"id":{"$get":"request.id"}, "enabled":true});
    tool["request"]["query"] =
        json!({"q":{"$get":"request.id"}, "absent":{"$omitIfNull":{"$get":"request.missing"}}});
    tool["request"]["headers"] = json!({"X-Feature":{"$get":"config.feature"}});
    tool["response"] = json!({"result":{"$get":"session.response.value"}});
    let mut cfg = config(value);
    cfg.configuration = json!({"feature":"test"});
    let plugin = CompiledConnector::compile(cfg).unwrap();
    assert_eq!(
        plugin
            .call("lookup", json!({"id":"hello world"}), context())
            .await
            .unwrap(),
        json!({"id":"hello world", "query":{"q":"hello world"}, "body":{"id":"hello world", "enabled":true}, "header":"test"})
    );
}

#[tokio::test]
async fn path_argument_cannot_escape_its_segment() {
    let plugin = CompiledConnector::compile(config(manifest("http://127.0.0.1:1"))).unwrap();
    for id in ["..", ".", "a/b", "a\\b", "%2e%2e", "a?b", "a#b", "a\r\nb"] {
        assert!(
            matches!(
                plugin.call("lookup", json!({"id":id}), context()).await,
                Err(ConnectorError::Arguments)
            ),
            "{id}"
        );
    }
}

#[tokio::test]
async fn public_plugin_does_not_forward_a_supplied_user_token() {
    let server = serve(Router::new().route(
        "/records/{id}",
        get(|headers: HeaderMap| async move {
            Json(json!({"has_token":headers.contains_key("authorization")}))
        }),
    ))
    .await;
    let mut value = manifest(&server.base);
    value["auth"] = json!("none");
    let plugin = CompiledConnector::compile(config(value)).unwrap();
    assert_eq!(
        plugin
            .call("lookup", json!({"id":"a"}), context())
            .await
            .unwrap(),
        json!({"has_token":false})
    );
}

#[tokio::test]
async fn redirects_are_not_followed_and_vendor_errors_are_redacted() {
    let hits = Arc::new(AtomicUsize::new(0));
    let target = serve(Router::new().route(
        "/leak",
        get({
            let hits = hits.clone();
            move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    "leaked"
                }
            }
        }),
    ))
    .await;
    let destination = format!("{}/leak", target.base);
    let server = serve(Router::new().route(
        "/records/{id}",
        get(move |Path(id): Path<String>| {
            let destination = destination.clone();
            async move {
                if id == "redirect" {
                    (
                        StatusCode::FOUND,
                        [("location", destination)],
                        "secret-token",
                    )
                        .into_response()
                } else {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        [("retry-after", "12")],
                        "secret-token",
                    )
                        .into_response()
                }
            }
        }),
    ))
    .await;
    let plugin = CompiledConnector::compile(config(manifest(&server.base))).unwrap();
    assert!(matches!(
        plugin
            .call("lookup", json!({"id":"redirect"}), context())
            .await,
        Err(ConnectorError::Http { status: 302, .. })
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    let error = plugin
        .call("lookup", json!({"id":"rate"}), context())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ConnectorError::Http {
            status: 429,
            retry_after_seconds: Some(12)
        }
    ));
    assert!(!format!("{error:?}").contains("secret-token"));
}

#[tokio::test]
async fn pagination_is_bounded_same_origin_and_preserves_base_path() {
    let server = serve(Router::new().route(
        "/v1/records/{id}",
        get(|Path(id): Path<String>, headers: HeaderMap| async move {
            let host = headers["host"].to_str().unwrap();
            Json(match id.as_str() {
                "a" => json!({"value":[1],"next":format!("http://{host}/v1/records/b")}),
                "b" => json!({"value":[2]}),
                "loop" => json!({"value":[1],"next":"/v1/records/loop"}),
                "cross" => json!({"value":[],"next":"https://evil.example/collect"}),
                "escape" => json!({"value":[],"next":"/other/path"}),
                _ => json!({"value":[],"next":123}),
            })
        }),
    ))
    .await;
    let mut value = manifest(&format!("{}/v1", server.base));
    value["tools"][0]["response"] =
        json!({"pagination":{"items_pointer":"/value","next_pointer":"/next","max_pages":2}});
    let plugin = CompiledConnector::compile(config(value)).unwrap();
    assert_eq!(
        plugin
            .call("lookup", json!({"id":"a"}), context())
            .await
            .unwrap(),
        json!([1, 2])
    );
    assert!(matches!(
        plugin.call("lookup", json!({"id":"loop"}), context()).await,
        Err(ConnectorError::Pagination)
    ));
    for id in ["cross", "escape"] {
        assert!(matches!(
            plugin.call("lookup", json!({"id":id}), context()).await,
            Err(ConnectorError::Destination)
        ));
    }
    assert!(matches!(
        plugin.call("lookup", json!({"id":"bad"}), context()).await,
        Err(ConnectorError::Response)
    ));
}

#[tokio::test]
async fn non_json_oversized_and_empty_responses_are_handled() {
    async fn reply(Path(id): Path<String>) -> Response {
        match id.as_str() {
            "empty" => StatusCode::NO_CONTENT.into_response(),
            "large" => "x".repeat(1024 * 1024 + 1).into_response(),
            _ => "not-json".into_response(),
        }
    }
    let server = serve(Router::new().route("/records/{id}", get(reply))).await;
    let plugin = CompiledConnector::compile(config(manifest(&server.base))).unwrap();
    assert_eq!(
        plugin
            .call("lookup", json!({"id":"empty"}), context())
            .await
            .unwrap(),
        Value::Null
    );
    assert!(matches!(
        plugin
            .call("lookup", json!({"id":"large"}), context())
            .await,
        Err(ConnectorError::ResponseTooLarge)
    ));
    assert!(matches!(
        plugin
            .call("lookup", json!({"id":"invalid"}), context())
            .await,
        Err(ConnectorError::Response)
    ));
}

#[tokio::test]
async fn dns_resolution_cannot_bypass_private_network_policy() {
    let server =
        serve(Router::new().route("/records/{id}", get(|| async { Json(json!({"ok":true})) })))
            .await;
    let base = server.base.replace("127.0.0.1", "localhost.localdomain");
    let mut cfg = config(manifest(&base));
    cfg.allow_private_network = false;
    let plugin = CompiledConnector::compile(cfg).unwrap();
    assert!(matches!(
        plugin.call("lookup", json!({"id":"a"}), context()).await,
        Err(ConnectorError::Transport)
    ));
}

#[tokio::test]
async fn write_requests_are_never_automatically_retried() {
    let hits = Arc::new(AtomicUsize::new(0));
    let server = serve(
        Router::new()
            .route(
                "/records/{id}",
                post(|State(hits): State<Arc<AtomicUsize>>| async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::SERVICE_UNAVAILABLE
                }),
            )
            .with_state(hits.clone()),
    )
    .await;
    let mut value = manifest(&server.base);
    value["tools"][0]["read_only"] = json!(false);
    value["tools"][0]["request"]["method"] = json!("POST");
    let plugin = CompiledConnector::compile(config(value)).unwrap();
    assert!(matches!(
        plugin.call("lookup", json!({"id":"a"}), context()).await,
        Err(ConnectorError::Http { status: 503, .. })
    ));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn json_provider_errors_do_not_masquerade_as_success() {
    let server = serve(Router::new().route(
        "/records/{id}",
        get(|| async { Json(json!({"failed":true,"error":"do-not-expose-vendor-secret"})) }),
    ))
    .await;
    let mut value = manifest(&server.base);
    value["tools"][0]["response"] = json!({"error_if":{"$get":"session.response.failed"}});
    let plugin = CompiledConnector::compile(config(value)).unwrap();
    let error = plugin
        .call("lookup", json!({"id":"a"}), context())
        .await
        .unwrap_err();
    assert!(matches!(error, ConnectorError::ProviderRejected));
    assert!(!error.to_string().contains("do-not-expose"));
}

#[tokio::test]
async fn concurrent_calls_are_bounded_and_cancellation_releases_capacity() {
    let entered = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = tokio::sync::watch::channel(false);
    let server = serve(Router::new().route(
        "/records/{id}",
        get({
            let entered = entered.clone();
            move || {
                let entered = entered.clone();
                let mut receiver = receiver.clone();
                async move {
                    entered.fetch_add(1, Ordering::SeqCst);
                    if !*receiver.borrow_and_update() {
                        let _ = receiver.changed().await;
                    }
                    Json(json!({"ok":true}))
                }
            }
        }),
    ))
    .await;
    let plugin = Arc::new(CompiledConnector::compile(config(manifest(&server.base))).unwrap());
    let mut pending = Vec::new();
    for _ in 0..16 {
        let plugin = plugin.clone();
        pending.push(tokio::spawn(async move {
            plugin.call("lookup", json!({"id":"a"}), context()).await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while entered.load(Ordering::SeqCst) < 16 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        plugin.call("lookup", json!({"id":"a"}), context()).await,
        Err(ConnectorError::Busy)
    ));
    for task in pending {
        task.abort();
        let _ = task.await;
    }
    sender.send(true).unwrap();
    assert_eq!(
        plugin
            .call("lookup", json!({"id":"a"}), context())
            .await
            .unwrap(),
        json!({"ok":true})
    );
}
