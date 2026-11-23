// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::{
    config::setting::{AttachmentSettings, MAX_ATTACHMENTS_PER_REQUEST},
    dto::{
        chat_stream::MessageRequest,
        files::{Attachment, File},
        models::GlobalAttachments,
        prompt::Prompt,
    },
    error::AppError,
    services::file_storage::{
        read_attachments_for_user, stored_attachments, stored_attachments_size,
    },
    state::SharedState,
};

// Image generation reads stored IDs even when the caller supplies inline bytes. Validate
// those IDs too, before the request can create a conversation or persist a message.
pub async fn validate_incoming_attachments(
    state: &SharedState,
    user_id: Uuid,
    messages: &[MessageRequest],
    image_generation: bool,
) -> Result<(), AppError> {
    ensure_attachment_counts(messages)?;
    ensure_newest_message_accepted(state, user_id, messages).await?;
    let ids = incoming_stored_ids(messages, image_generation);
    if stored_attachments_size(&state.database, user_id, &ids).await? > MAX_REQUEST_ATTACHMENT_BYTES
    {
        return Err(AppError::ValidationInvalidField { field: "files" });
    }
    read_attachments_for_user(
        &state.database,
        &state.settings.file_storage_root,
        user_id,
        &ids,
    )
    .await?;
    Ok(())
}

const MAX_REQUEST_ATTACHMENT_BYTES: u64 = 200 * 1024 * 1024;

pub fn global_attachments(settings: &AttachmentSettings) -> GlobalAttachments {
    GlobalAttachments {
        accepted_types: settings.accepted_types.clone(),
        max_file_size_bytes: settings.max_file_bytes,
        max_files_per_message: settings.max_files_per_message,
    }
}

// Platform limits apply to what the user sends now; earlier turns were accepted under the
// limits in force at the time and are read from history.
async fn ensure_newest_message_accepted(
    state: &SharedState,
    user_id: Uuid,
    messages: &[MessageRequest],
) -> Result<(), AppError> {
    let policy = &state.settings.attachments;
    let Some(message) = messages.last() else {
        return Ok(());
    };
    if message.files.len() > policy.max_files_per_message {
        return Err(AppError::ValidationInvalidField { field: "files" });
    }
    let stored_ids = message
        .files
        .iter()
        .filter(|file| file.base64.is_none())
        .map(|file| file.id)
        .collect::<Vec<_>>();
    let stored = stored_attachments(&state.database, user_id, &stored_ids).await?;
    let inline = message.files.iter().filter_map(|file| {
        file.base64.as_deref().map(|data| {
            (
                inline_size(data),
                file.content_type.as_str(),
                file.name.as_str(),
            )
        })
    });
    let all_accepted = stored
        .iter()
        .map(|file| {
            (
                file.size.max(0) as u64,
                file.content_type.as_str(),
                file.name.as_str(),
            )
        })
        .chain(inline)
        .all(|(size, content_type, name)| {
            size <= policy.max_file_bytes
                && accepts_type(&policy.accepted_types, content_type, name)
        });
    if all_accepted {
        Ok(())
    } else {
        Err(AppError::ValidationInvalidField { field: "files" })
    }
}

fn inline_size(data: &str) -> u64 {
    let payload = data.rsplit_once(',').map_or(data, |(_, payload)| payload);
    let encoded = payload
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'=')
        .count() as u64;
    encoded * 3 / 4
}

fn accepts_type(accepted: &[String], content_type: &str, name: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let name = name.to_lowercase();
    accepted.iter().any(|token| {
        if token == "*/*" {
            true
        } else if token.starts_with('.') {
            name.len() > token.len() && name.ends_with(token.as_str())
        } else if let Some(top) = token.strip_suffix("/*") {
            mime.split_once('/').is_some_and(|(kind, _)| kind == top)
        } else {
            mime == *token
        }
    })
}

fn ensure_attachment_counts(messages: &[MessageRequest]) -> Result<(), AppError> {
    let mut total = 0;
    for message in messages {
        total += message.files.len();
        let mut seen = HashSet::with_capacity(message.files.len());
        if total > MAX_ATTACHMENTS_PER_REQUEST
            || !message.files.iter().all(|file| seen.insert(file.id))
        {
            return Err(AppError::ValidationInvalidField { field: "files" });
        }
    }
    Ok(())
}

fn incoming_stored_ids(messages: &[MessageRequest], image_generation: bool) -> Vec<Uuid> {
    messages
        .iter()
        .flat_map(|message| &message.files)
        .filter(|file| image_generation || file.base64.is_none())
        .map(|file| file.id)
        .collect()
}

pub async fn hydrate_files(
    files: &mut [File],
    user_id: Uuid,
    state: &SharedState,
) -> Result<(), AppError> {
    let ids = files
        .iter()
        .filter(|file| file.base64.is_none())
        .map(|file| file.id)
        .collect::<Vec<_>>();
    let attachments = read_attachments_for_user(
        &state.database,
        &state.settings.file_storage_root,
        user_id,
        &ids,
    )
    .await?;
    hydrate_files_from(&attachments, files)
}

fn hydrate_files_from(
    attachments: &HashMap<Uuid, Attachment>,
    files: &mut [File],
) -> Result<(), AppError> {
    for file in files {
        if file.base64.is_some() {
            continue;
        }
        let attachment = attachments
            .get(&file.id)
            .ok_or(AppError::ResourceNotFound)?;
        file.base64 = attachment.get_base64();
        if file.base64.is_none() {
            return Err(AppError::ResourceNotFound);
        }
    }
    Ok(())
}

pub async fn hydrate_prompt_files(
    prompts: &mut [Prompt],
    user_id: Uuid,
    state: &SharedState,
) -> Result<(), AppError> {
    let ids = prompts
        .iter()
        .flat_map(|prompt| &prompt.files)
        .filter(|file| file.base64.is_none())
        .map(|file| file.id)
        .collect::<Vec<_>>();
    let attachments = read_attachments_for_user(
        &state.database,
        &state.settings.file_storage_root,
        user_id,
        &ids,
    )
    .await?;
    for prompt in prompts {
        hydrate_files_from(&attachments, &mut prompt.files)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::messages::ChatRole;

    fn file(id: Uuid, inline: bool) -> File {
        File {
            id,
            size: None,
            name: "file.txt".into(),
            content_type: "text/plain".into(),
            openai_id: None,
            base64: inline.then(|| "YQ==".into()),
        }
    }

    #[test]
    fn every_message_is_validated_before_any_is_saved() {
        let stored = Uuid::new_v4();
        let inline = Uuid::new_v4();
        let messages = vec![
            MessageRequest {
                role: ChatRole::User,
                content: "first".into(),
                files: vec![],
            },
            MessageRequest {
                role: ChatRole::User,
                content: "second".into(),
                files: vec![file(stored, false), file(inline, true)],
            },
        ];
        assert_eq!(incoming_stored_ids(&messages, false), vec![stored]);
        assert_eq!(incoming_stored_ids(&messages, true), vec![stored, inline]);
    }

    fn message(files: Vec<File>) -> MessageRequest {
        MessageRequest {
            role: ChatRole::User,
            content: "hi".into(),
            files,
        }
    }

    #[test]
    fn repeated_file_ids_in_one_message_are_rejected() {
        let id = Uuid::new_v4();
        assert!(matches!(
            ensure_attachment_counts(&[message(vec![file(id, false), file(id, false)])]),
            Err(AppError::ValidationInvalidField { field: "files" })
        ));
        assert!(ensure_attachment_counts(&[message(vec![file(id, false)])]).is_ok());
    }

    #[test]
    fn attachment_count_is_capped_across_the_request() {
        let files = |n: usize| (0..n).map(|_| file(Uuid::new_v4(), false)).collect();
        assert!(ensure_attachment_counts(&[message(files(MAX_ATTACHMENTS_PER_REQUEST))]).is_ok());
        assert!(
            ensure_attachment_counts(&[
                message(files(MAX_ATTACHMENTS_PER_REQUEST)),
                message(files(1))
            ])
            .is_err()
        );
    }

    #[test]
    fn hydration_refuses_unknown_ids() {
        let mut files = vec![file(Uuid::new_v4(), false)];
        assert!(matches!(
            hydrate_files_from(&HashMap::new(), &mut files),
            Err(AppError::ResourceNotFound)
        ));
    }

    #[test]
    fn hydration_preserves_inline_inputs_and_uses_stored_bytes_for_references() {
        let id = Uuid::new_v4();
        let mut files = vec![file(id, false), file(Uuid::new_v4(), true)];
        let attachments = HashMap::from([(
            id,
            Attachment {
                file: Some(b"stored".to_vec()),
                name: "stored.txt".into(),
                content_type: "text/plain".into(),
            },
        )]);
        hydrate_files_from(&attachments, &mut files).expect("hydrate");
        assert_eq!(files[0].base64.as_deref(), Some("c3RvcmVk"));
        assert_eq!(files[1].base64.as_deref(), Some("YQ=="));
    }

    #[test]
    fn accepted_types_match_by_mime_wildcard_and_extension() {
        let accepted = ["image/*", "application/pdf", ".tar.gz", ".txt"]
            .map(String::from)
            .to_vec();
        assert!(accepts_type(&accepted, "image/PNG", "a.png"));
        assert!(accepts_type(
            &accepted,
            "application/pdf; charset=binary",
            "report"
        ));
        assert!(accepts_type(
            &accepted,
            "application/octet-stream",
            "Backup.TAR.GZ"
        ));
        assert!(accepts_type(
            &accepted,
            "application/octet-stream",
            "notes.txt"
        ));
        assert!(!accepts_type(&accepted, "application/octet-stream", ".txt"));
        assert!(!accepts_type(&accepted, "application/zip", "archive.zip"));
        assert!(!accepts_type(&accepted, "imagex/png", "a.bin"));
        assert!(accepts_type(&["*/*".to_string()], "", "anything"));
    }

    #[test]
    fn inline_size_estimates_decoded_bytes_with_or_without_a_data_url_prefix() {
        assert_eq!(inline_size("aGVsbG8gd29ybGQ="), 11);
        assert_eq!(inline_size("data:text/plain;base64,aGVsbG8gd29ybGQ="), 11);
        assert_eq!(inline_size("aGVs\nbG8="), 5);
    }

    #[test]
    fn global_attachments_publish_the_configured_limits_in_snake_case() {
        let settings = AttachmentSettings {
            accepted_types: vec!["image/*".into(), ".pdf".into()],
            max_file_bytes: 1024,
            max_files_per_message: 3,
        };
        assert_eq!(
            serde_json::to_value(global_attachments(&settings)).unwrap(),
            serde_json::json!({
                "accepted_types": ["image/*", ".pdf"],
                "max_file_size_bytes": 1024,
                "max_files_per_message": 3
            })
        );
    }
}
