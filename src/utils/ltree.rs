// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use uuid::Uuid;

pub fn ltree_label_from_uuid(id: Uuid) -> String {
    id.simple().to_string()
}
