// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DeploymentHealth {
    pub status: &'static str,
    pub version: &'static str,
    pub migration_head: Option<String>,
    pub expected_migration_head: &'static str,
}

impl DeploymentHealth {
    pub fn is_ready(&self) -> bool {
        self.migration_head.as_deref() == Some(self.expected_migration_head)
    }
}
