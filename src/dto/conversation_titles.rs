// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde_json::Value;
use uuid::Uuid;

use crate::dto::files::File;

pub struct NewConversation {
    pub user_id: Uuid,
    pub department_id: Option<Uuid>,
    pub provider: String,
    pub model: String,
    pub prompt: String,
    pub files: Vec<File>,
    pub metadata: Value,
    pub is_image_gen: bool,
    pub via_gateway: bool,
    pub input_rate: Option<f64>,
    pub output_rate: Option<f64>,
}
