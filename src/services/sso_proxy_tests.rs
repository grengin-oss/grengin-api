// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use super::sso_egress::{SsoEgressError, SsoHttpClient};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
#[ignore = "subprocess helper; invoked with isolated proxy environment by the parent test"]
async fn environment_child() {
    let Ok(mode) = std::env::var("GRENGIN_SSO_TEST_MODE") else {
        return;
    };
    let url = std::env::var("GRENGIN_SSO_TEST_URL").unwrap();
    let root = if mode == "untrusted" {
        None
    } else {
        Some(
            reqwest::Certificate::from_pem(
                &std::fs::read(std::env::var("GRENGIN_SSO_TEST_CA").unwrap()).unwrap(),
            )
            .unwrap(),
        )
    };
    let client = SsoHttpClient::with_test_root(root).unwrap();
    let response = client.send(client.get(url)).await;
    match mode.as_str() {
        "blocked" | "direct_dns_missing" | "bypass_dns_missing" => {
            assert!(matches!(response, Err(SsoEgressError::Destination)))
        }
        "untrusted" => assert!(matches!(response, Err(SsoEgressError::Request(_)))),
        "oversized" => assert!(matches!(response, Err(SsoEgressError::Body(_)))),
        "redirect" => assert_eq!(response.unwrap().status, reqwest::StatusCode::FOUND),
        "http" => assert_eq!(
            response.unwrap().json::<serde_json::Value>().unwrap()["proxy"],
            true
        ),
        _ => assert_eq!(
            response.unwrap().json::<serde_json::Value>().unwrap()["secure"],
            true
        ),
    }
}

struct TlsOrigin {
    address: std::net::SocketAddr,
    certificate: Vec<u8>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl TlsOrigin {
    fn new() -> Self {
        use openssl::{
            asn1::Asn1Time,
            ec::{EcGroup, EcKey},
            hash::MessageDigest,
            nid::Nid,
            pkey::PKey,
            ssl::{SslAcceptor, SslMethod},
            x509::{
                X509, X509NameBuilder,
                extension::{BasicConstraints, SubjectAlternativeName},
            },
        };
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let serial = openssl::bn::BigNum::from_u32(1)
            .unwrap()
            .to_asn1_integer()
            .unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.append_extension(BasicConstraints::new().critical().ca().build().unwrap())
            .unwrap();
        let san = SubjectAlternativeName::new()
            .ip("127.0.0.1")
            .dns("proxy-only.invalid")
            .build(&cert.x509v3_context(None, None))
            .unwrap();
        cert.append_extension(san).unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        let cert = cert.build();
        let certificate = cert.to_pem().unwrap();
        let mut tls = SslAcceptor::mozilla_intermediate(SslMethod::tls()).unwrap();
        tls.set_private_key(&key).unwrap();
        tls.set_certificate(&cert).unwrap();
        let tls = tls.build();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::SeqCst) {
                let Ok((socket, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let Ok(mut socket) = tls.accept(socket) else {
                    continue;
                };
                let mut request = Vec::new();
                let mut byte = [0];
                while request.len() < 8192 && socket.read(&mut byte).unwrap_or(0) == 1 {
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let (status, headers, body) = if request.starts_with("GET /redirect ") {
                    (
                        "302 Found",
                        "Location: http://169.254.169.254/latest/meta-data\r\n",
                        Vec::new(),
                    )
                } else if request.starts_with("GET /oversized ") {
                    ("200 OK", "", vec![b' '; 1024 * 1024 + 1])
                } else {
                    ("200 OK", "", br#"{"secure":true}"#.to_vec())
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
                let _ = socket.write_all(&body);
            }
        });
        Self {
            address,
            certificate,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for TlsOrigin {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standard_proxies_preserve_https_verification_bypass_and_egress_guards() {
    let origin = TlsOrigin::new();
    let root = std::env::temp_dir().join(format!("grengin-sso-proxy-{}.pem", uuid::Uuid::new_v4()));
    std::fs::write(&root, &origin.certificate).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("http://{}", listener.local_addr().unwrap());
    let calls = Arc::new(AtomicUsize::new(0));
    let received = calls.clone();
    let authority = origin.address.to_string();
    let proxy = tokio::spawn(async move {
        loop {
            let (mut downstream, _) = listener.accept().await.unwrap();
            let received = received.clone();
            let authority = authority.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0];
                while head.len() < 8192 && downstream.read(&mut byte).await.unwrap_or(0) == 1 {
                    head.push(byte[0]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                received.fetch_add(1, Ordering::SeqCst);
                if String::from_utf8_lossy(&head).starts_with("CONNECT ") {
                    let proxy_only = format!(
                        "proxy-only.invalid:{}",
                        authority.rsplit(':').next().unwrap()
                    );
                    assert!(
                        String::from_utf8_lossy(&head)
                            .starts_with(&format!("CONNECT {authority} "))
                            || String::from_utf8_lossy(&head)
                                .starts_with(&format!("CONNECT {proxy_only} "))
                    );
                    let mut upstream = tokio::net::TcpStream::connect(&authority).await.unwrap();
                    downstream
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
                } else {
                    downstream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{\"proxy\":true}").await.unwrap();
                }
            });
        }
    });
    for (mode, path, delta) in [
        ("https", "/ok", 1),
        ("lowercase", "/ok", 1),
        ("all_proxy", "/ok", 1),
        ("proxy_dns_only", "/ok", 1),
        ("direct_dns_missing", "/ok", 0),
        ("bypass_dns_missing", "/ok", 0),
        ("http", "", 1),
        ("bypass", "/ok", 0),
        ("untrusted", "/ok", 1),
        ("redirect", "/redirect", 1),
        ("oversized", "/oversized", 1),
        ("blocked", "", 0),
    ] {
        let before = calls.load(Ordering::SeqCst);
        let url = match mode {
            "http" => "http://127.0.0.1:1/profile".to_string(),
            "blocked" => "https://169.254.169.254/latest/meta-data".to_string(),
            "proxy_dns_only" | "direct_dns_missing" | "bypass_dns_missing" => {
                format!("https://proxy-only.invalid:{}{path}", origin.address.port())
            }
            _ => format!("https://{}{path}", origin.address),
        };
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command.args([
            "--ignored",
            "--exact",
            "services::sso_proxy_tests::environment_child",
        ]);
        for variable in [
            "REQUEST_METHOD",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            command.env_remove(variable);
        }
        command.env(
            "HTTP_PROXY",
            if mode == "http" {
                &proxy_url
            } else {
                "http://127.0.0.1:1"
            },
        );
        command.env("HTTPS_PROXY", &proxy_url);
        if mode == "direct_dns_missing" {
            command.env_remove("HTTPS_PROXY");
            command.env_remove("HTTP_PROXY");
        }
        if mode == "bypass_dns_missing" {
            command.env("NO_PROXY", ".invalid");
        }
        if mode == "lowercase" || mode == "all_proxy" {
            command.env_remove("HTTPS_PROXY");
            command.env_remove("HTTP_PROXY");
            command.env(
                if mode == "lowercase" {
                    "https_proxy"
                } else {
                    "ALL_PROXY"
                },
                &proxy_url,
            );
        }
        if mode == "bypass" {
            command.env("NO_PROXY", "127.0.0.1");
        }
        command
            .env("GRENGIN_SSO_TEST_MODE", mode)
            .env("GRENGIN_SSO_TEST_URL", url)
            .env("GRENGIN_SSO_TEST_CA", &root);
        let output = tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(calls.load(Ordering::SeqCst) - before, delta, "{mode}");
    }
    proxy.abort();
    std::fs::remove_file(root).unwrap();
}
