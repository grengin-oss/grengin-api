// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&schemars::schema_for!(
            connector_server_plugin::ConnectorManifest
        ))
        .expect("schema serialization")
    );
}
