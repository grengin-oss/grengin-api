// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use reqwest::{Client, ClientBuilder, redirect::Policy};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HttpTimeouts {
    pub connect: Duration,
    pub read: Option<Duration>,
    pub total: Option<Duration>,
}

impl HttpTimeouts {
    pub const SHORT_CALL: Self = Self {
        connect: Duration::from_secs(10),
        read: Some(Duration::from_secs(10)),
        total: Some(Duration::from_secs(15)),
    };

    // MCP sessions keep SSE streams open and tool calls may run for minutes without sending
    // bytes, so only the connect phase can be bounded safely.
    pub const LONG_LIVED_STREAM: Self = Self {
        connect: Duration::from_secs(10),
        read: None,
        total: None,
    };

    pub fn apply(self, builder: ClientBuilder) -> ClientBuilder {
        let builder = builder.connect_timeout(self.connect);
        let builder = match self.read {
            Some(read) => builder.read_timeout(read),
            None => builder,
        };
        match self.total {
            Some(total) => builder.timeout(total),
            None => builder,
        }
    }
}

pub fn short_call_client() -> reqwest::Result<Client> {
    HttpTimeouts::SHORT_CALL
        .apply(Client::builder().redirect(Policy::none()))
        .build()
}

pub fn long_lived_stream_client() -> reqwest::Result<Client> {
    HttpTimeouts::LONG_LIVED_STREAM
        .apply(Client::builder())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tokio::{io::AsyncReadExt, net::TcpListener};

    #[test]
    fn short_calls_are_bounded_end_to_end() {
        let timeouts = HttpTimeouts::SHORT_CALL;
        let total = timeouts.total.expect("short calls need a total timeout");
        assert!(timeouts.connect <= total);
        assert!(timeouts.read.expect("short calls need a read timeout") <= total);
    }

    #[test]
    fn long_lived_streams_never_get_a_total_or_idle_timeout() {
        let timeouts = HttpTimeouts::LONG_LIVED_STREAM;
        assert_eq!(timeouts.total, None);
        assert_eq!(timeouts.read, None);
        assert!(timeouts.connect <= Duration::from_secs(10));
    }

    #[test]
    fn both_shared_clients_build() {
        short_call_client().expect("short call client");
        long_lived_stream_client().expect("long lived client");
    }

    #[tokio::test]
    async fn read_timeout_aborts_a_server_that_goes_silent() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer).await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = HttpTimeouts {
            connect: Duration::from_secs(1),
            read: Some(Duration::from_millis(200)),
            total: None,
        }
        .apply(Client::builder())
        .build()
        .expect("client");

        let started = Instant::now();
        let error = client
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect_err("silent server must time out");

        assert!(error.is_timeout());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
