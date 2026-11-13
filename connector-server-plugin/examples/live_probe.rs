// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::io::Read;

use connector_server_plugin::{CallContext, CompiledConnector, ConnectorPluginConfig};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Probe {
    config: ConnectorPluginConfig,
    tool: String,
    arguments: Value,
    access_token: String,
    user_id: String,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut input)?;
    if input.len() > 1024 * 1024 {
        return Err("probe input exceeds limit".into());
    }
    let mut probe: Probe = serde_json::from_slice(&input).map_err(|_| "invalid probe input")?;
    probe.config.allow_insecure_http = false;
    probe.config.allow_private_network = false;
    let connector = CompiledConnector::compile(probe.config)?;
    match connector
        .call(
            &probe.tool,
            probe.arguments,
            CallContext {
                user_id: &probe.user_id,
                access_token: Some(&probe.access_token),
            },
        )
        .await
    {
        Ok(value) => {
            let (shape, size) = match value {
                Value::Array(items) => ("array", items.len()),
                Value::Object(items) => ("object", items.len()),
                Value::Null => ("null", 0),
                _ => ("scalar", 1),
            };
            println!("{}", json!({"success":true,"shape":shape,"size":size}));
        }
        Err(error) => {
            println!("{}", json!({"success":false,"error":error.to_string()}));
            std::process::exit(1);
        }
    }
    Ok(())
}
