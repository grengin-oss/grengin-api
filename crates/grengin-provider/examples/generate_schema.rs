// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{env, fs, path::PathBuf};

use grengin_provider::ProviderManifestV1;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from("crates/grengin-provider/schema/provider-plugin-v1.schema.json")
    });
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let schema = schemars::schema_for!(ProviderManifestV1);
    let mut json = serde_json::to_string_pretty(&schema)?;
    json.push('\n');
    fs::write(&output, json)?;
    println!("wrote {}", output.display());
    Ok(())
}
