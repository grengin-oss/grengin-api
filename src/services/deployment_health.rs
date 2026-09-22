// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::dto::deployment::DeploymentHealth;
use sea_orm::DatabaseConnection;

pub async fn load_deployment_health(db: &DatabaseConnection) -> DeploymentHealth {
    let expected = migration::migration_head();
    let applied = migration::applied_migration_head(db).await;
    let ready = matches!(&applied, Ok(Some(head)) if head == expected);
    if let Err(error) = &applied {
        eprintln!("migration health lookup failed: {error}");
    }

    DeploymentHealth {
        status: if ready { "Okay" } else { "Degraded" },
        version: env!("CARGO_PKG_VERSION"),
        migration_head: applied.ok().flatten(),
        expected_migration_head: expected,
    }
}

#[cfg(test)]
mod tests {
    use super::DeploymentHealth;

    #[test]
    fn readiness_requires_the_applied_and_expected_heads_to_match() {
        let expected = migration::migration_head();
        let ready = DeploymentHealth {
            status: "Okay",
            version: env!("CARGO_PKG_VERSION"),
            migration_head: Some(expected.to_string()),
            expected_migration_head: expected,
        };
        let stale = DeploymentHealth {
            status: "Degraded",
            version: env!("CARGO_PKG_VERSION"),
            migration_head: Some("m_old".to_string()),
            expected_migration_head: expected,
        };

        assert!(ready.is_ready());
        assert!(!stale.is_ready());
    }
}
