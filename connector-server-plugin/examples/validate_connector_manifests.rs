// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use connector_server_plugin::{CompiledConnector, ConnectorPluginConfig};
use std::{fs, path::Path, process::ExitCode};

fn validate(path: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    if fs::symlink_metadata(path)?.is_symlink() {
        return Err(format!("symlinks are not allowed: {}", path.display()).into());
    }
    if path.is_dir() {
        let mut checked = 0;
        for entry in fs::read_dir(path)? {
            checked += validate(&entry?.path())?;
        }
        return Ok(checked);
    }
    if path.file_name().is_none_or(|name| name != "plugin.json") {
        return Ok(0);
    }
    let manifest = serde_json::from_slice(&fs::read(path)?)?;
    let compiled = CompiledConnector::compile(ConnectorPluginConfig {
        manifest,
        ..Default::default()
    })?;
    println!(
        "valid: {}@{} {}",
        compiled.manifest().id,
        compiled.manifest().version,
        path.display()
    );
    Ok(1)
}

fn main() -> ExitCode {
    let mut checked = 0;
    for path in std::env::args_os().skip(1) {
        match validate(Path::new(&path)) {
            Ok(count) => checked += count,
            Err(error) => {
                eprintln!("invalid connector plugin: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    if checked == 0 {
        eprintln!(
            "usage: validate_connector_manifests <plugin.json or directory>; at least one manifest required"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
