// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{claims::Claims, error::AuthError, permissions::PERMISSION_ANALYTICS_VIEW},
    dto::analytics::{DepartmentAnalytics, DepartmentAnalyticsQuery},
    services::{
        analytics, analytics_cache,
        authorization::{AuthorizationService, PermissionScopeMode},
        me_helpers::{load_administered_department_ids, load_administered_department_paths},
    },
    state::SharedState,
};

#[derive(Debug, PartialEq, Eq)]
enum DepartmentAnalyticsScope {
    OrgWide,
    Departments(Vec<String>),
}

impl DepartmentAnalyticsScope {
    fn for_caller(org_wide: bool, administered_paths: Vec<String>) -> Self {
        if org_wide {
            Self::OrgWide
        } else {
            Self::Departments(administered_paths)
        }
    }
}

pub async fn department_analytics_for_caller(
    claims: Claims,
    app_state: &SharedState,
    query: DepartmentAnalyticsQuery,
) -> Result<DepartmentAnalytics, AuthError> {
    let org_wide = AuthorizationService::new(&app_state.database)
        .user_has_permission(
            claims.user_id,
            PERMISSION_ANALYTICS_VIEW,
            None,
            PermissionScopeMode::RequireOrgWide,
        )
        .await?;
    let administered_paths = if org_wide {
        Vec::new()
    } else {
        let department_ids = load_administered_department_ids(claims, app_state).await?;
        load_administered_department_paths(app_state, &department_ids).await?
    };

    let result = match DepartmentAnalyticsScope::for_caller(org_wide, administered_paths) {
        DepartmentAnalyticsScope::OrgWide => {
            analytics_cache::get_department_analytics_cached(&app_state.database, query).await
        }
        DepartmentAnalyticsScope::Departments(scope_paths) => {
            analytics::get_department_analytics_scoped(&app_state.database, query, &scope_paths)
                .await
        }
    };
    result.map_err(|e| {
        eprintln!("Department analytics error: {e}");
        AuthError::DbTimeout
    })
}

#[cfg(test)]
mod tests {
    use super::DepartmentAnalyticsScope;

    #[test]
    fn org_wide_admin_gets_all_departments() {
        assert_eq!(
            DepartmentAnalyticsScope::for_caller(true, vec!["root.sales".to_string()]),
            DepartmentAnalyticsScope::OrgWide
        );
    }

    #[test]
    fn department_scoped_admin_is_limited_to_administered_departments() {
        let paths = vec!["root.sales".to_string(), "root.hr".to_string()];
        assert_eq!(
            DepartmentAnalyticsScope::for_caller(false, paths.clone()),
            DepartmentAnalyticsScope::Departments(paths)
        );
    }

    #[test]
    fn scoped_admin_without_administered_departments_gets_an_empty_scope() {
        assert_eq!(
            DepartmentAnalyticsScope::for_caller(false, Vec::new()),
            DepartmentAnalyticsScope::Departments(Vec::new())
        );
    }
}
