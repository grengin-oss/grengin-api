// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::{
    dto::{
        chat_stream::MessageRequest,
        files::{Attachment, File},
        prompt::Prompt,
    },
    error::AppError,
    services::file_storage::{read_attachments_for_user, stored_attachments_size},
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

const MAX_REQUEST_ATTACHMENTS: usize = 32;
const MAX_REQUEST_ATTACHMENT_BYTES: u64 = 200 * 1024 * 1024;

fn ensure_attachment_counts(messages: &[MessageRequest]) -> Result<(), AppError> {
    let mut total = 0;
    for message in messages {
        total += message.files.len();
        let mut seen = HashSet::with_capacity(message.files.len());
        if total > MAX_REQUEST_ATTACHMENTS || !message.files.iter().all(|file| seen.insert(file.id))
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
        assert!(ensure_attachment_counts(&[message(files(MAX_REQUEST_ATTACHMENTS))]).is_ok());
        assert!(
            ensure_attachment_counts(&[message(files(MAX_REQUEST_ATTACHMENTS)), message(files(1))])
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
}
