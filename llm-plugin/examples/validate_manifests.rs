// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use llm_plugin::{
    ChatMessage, ChatRequest, ChatRole, ContentPart, DeclarativeProvider, MappingContext, ModelId,
    ProviderManifestV1, ProviderModelType, ProviderRuntimeConfig, ToolChoice, ToolDefinition,
    evaluate_mapping,
};
use serde_json::{Value, json};

fn main() -> ExitCode {
    let roots = env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if roots.is_empty() {
        eprintln!("usage: validate_manifests <plugin.json or directory> [...]");
        return ExitCode::from(2);
    }

    let mut files = Vec::new();
    for root in roots {
        if let Err(error) = collect_manifests(&root, &mut files) {
            eprintln!("{}: {error}", root.display());
            return ExitCode::FAILURE;
        }
    }
    files.sort();
    files.dedup();
    if files.is_empty() {
        eprintln!("no plugin.json files found");
        return ExitCode::FAILURE;
    }

    let mut failed = false;
    for file in files {
        match validate_manifest(&file) {
            Ok(id) => println!("valid: {id}\t{}", file.display()),
            Err(error) => {
                failed = true;
                eprintln!("invalid: {}: {error}", file.display());
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn collect_manifests(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    if root.is_file() {
        files.push(root.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_manifests(&path, files)?;
        } else if path.file_name().is_some_and(|name| name == "plugin.json") {
            files.push(path);
        }
    }
    Ok(())
}

fn validate_manifest(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let manifest = ProviderManifestV1::from_json(&bytes)?;
    probe_chat_payloads(&manifest)?;
    let credentials = manifest
        .credentials
        .iter()
        .map(|credential| {
            (
                credential.id.clone(),
                "catalog-validation-placeholder".to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let id = manifest.id.clone();
    DeclarativeProvider::new(
        manifest,
        ProviderRuntimeConfig {
            credentials,
            ..Default::default()
        },
    )?;
    Ok(id)
}

fn probe_chat_payloads(manifest: &ProviderManifestV1) -> Result<(), Box<dyn std::error::Error>> {
    let Some(operation) = manifest.operations.chat_stream.as_ref() else {
        return Ok(());
    };
    let Some(body) = operation.request.body.as_ref() else {
        return Ok(());
    };
    let text_models = manifest
        .models
        .iter()
        .filter(|model| model.model_type == ProviderModelType::TextGenerator)
        .collect::<Vec<_>>();
    let Some(default_model) = text_models.first() else {
        return Ok(());
    };

    evaluate_chat_payload(
        manifest,
        body,
        default_model.id.as_str(),
        Vec::new(),
        None,
        false,
    )?;

    if let Some(tool_model) = text_models.iter().find(|model| {
        model
            .capabilities
            .chat
            .as_ref()
            .is_some_and(|chat| chat.tools)
    }) {
        let tool = ToolDefinition {
            name: "catalog_probe".to_string(),
            description: Some("Validate named tool payload mapping".to_string()),
            parameters: json!({"type": "object", "properties": {}}),
        };
        evaluate_chat_payload(
            manifest,
            body,
            tool_model.id.as_str(),
            vec![tool],
            Some(ToolChoice::Named("catalog_probe".to_string())),
            false,
        )?;
    }

    for model in text_models.iter().filter(|model| {
        model
            .metadata
            .get("supportsWebSearch")
            .and_then(Value::as_bool)
            == Some(true)
    }) {
        evaluate_chat_payload(manifest, body, model.id.as_str(), Vec::new(), None, true)?;
    }
    Ok(())
}

fn evaluate_chat_payload(
    manifest: &ProviderManifestV1,
    body: &llm_plugin::MappingExpression,
    model: &str,
    tools: Vec<ToolDefinition>,
    tool_choice: Option<ToolChoice>,
    web_search: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = ChatRequest {
        model: ModelId::new(model),
        messages: vec![ChatMessage {
            role: ChatRole::User,
            content: vec![ContentPart::Text {
                text: "catalog validation probe".to_string(),
            }],
            tool_calls: Vec::new(),
            tool_result: None,
        }],
        temperature: Some(0.0),
        max_tokens: Some(64),
        tools,
        tool_choice,
        web_search,
        options: Value::Null,
    };
    let context = MappingContext::new(json!({
        "request": request,
        "session": {"toolRound": 0, "captures": {}}
    }));
    let mapped = evaluate_mapping(body, &context, &manifest.mappings)?;
    if !mapped.is_object() {
        return Err(
            format!("chat payload probe for model {model} did not produce an object").into(),
        );
    }
    Ok(())
}
