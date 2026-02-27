pub use sea_orm_migration::prelude::*;

mod m20251125_000001_create_users;
mod m20251125_000002_create_oauth_sessions;
mod m20251125_000003_create_conversations;
mod m20251125_000004_create_messages;
mod m20251125_000005_create_prompt_templates;
mod m20250201_000001_make_previous_message_id_nullable;
mod m20250201_000002_add_request_id_to_messages;
mod m20251211_000005_add_deleted_to_messages;
mod m20251216_000001_create_organizations;
mod m20251216_000002_add_org_id_to_users;
mod m20251218_000001_drop_users_email_unique;
mod m20251218_000001_create_ai_engines;
mod m20251218_000001_create_files;
mod m20251229_000001_create_sso_providers;
mod m20250102_000001_add_redirect_url_to_sso_providers;
mod m20250107_000001_create_usage_logs;
mod m20250107_000002_create_usage_summary_daily;
mod m20260111_000001_replace_org_with_branding;
mod m20260116_000002_create_departments;
mod m20260116_000003_users_department_id_fk;
mod m20260127_000003_add_department_budget_columns;
mod m20260130_000003_create_analytics;
mod m20260204_000001_add_department_budget_available;
mod m20260209_000001_add_authorization_layer;
mod m20260210_000001_update_permissions_ai_platform_sso;
mod m20260216_000001_mcp_revamp;
mod m20260217_000001_mcp_credentials;
mod m20260217_000002_mcp_url_rename;
mod m20260218_000001_fix_mcp_schema;
mod m20260227_000001_drop_legacy_user_role;
mod m20260227_000002_add_permission_description_key;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
          Box::new(m20251125_000001_create_users::Migration),
          Box::new(m20251125_000002_create_oauth_sessions::Migration),
          Box::new(m20251125_000003_create_conversations::Migration),
          Box::new(m20251125_000004_create_messages::Migration),
          Box::new(m20251125_000005_create_prompt_templates::Migration),
          Box::new(m20250201_000001_make_previous_message_id_nullable::Migration),
          Box::new(m20250201_000002_add_request_id_to_messages::Migration),
          Box::new(m20251211_000005_add_deleted_to_messages::Migration),
          Box::new(m20251216_000001_create_organizations::Migration),
          Box::new(m20251216_000002_add_org_id_to_users::Migration),
          Box::new(m20251218_000001_drop_users_email_unique::Migration),
          Box::new(m20251218_000001_create_ai_engines::Migration),
          Box::new(m20251218_000001_create_files::Migration),
          Box::new(m20251229_000001_create_sso_providers::Migration),
          Box::new(m20250102_000001_add_redirect_url_to_sso_providers::Migration),
          Box::new(m20250107_000001_create_usage_logs::Migration),
          Box::new(m20250107_000002_create_usage_summary_daily::Migration),
          Box::new(m20260111_000001_replace_org_with_branding::Migration),
          Box::new(m20260116_000002_create_departments::Migration),
          Box::new(m20260116_000003_users_department_id_fk::Migration),
          Box::new(m20260127_000003_add_department_budget_columns::Migration),
          Box::new(m20260130_000003_create_analytics::Migration),
          Box::new(m20260204_000001_add_department_budget_available::Migration),
          Box::new(m20260209_000001_add_authorization_layer::Migration),
          Box::new(m20260210_000001_update_permissions_ai_platform_sso::Migration),
          Box::new(m20260216_000001_mcp_revamp::Migration),
          Box::new(m20260217_000001_mcp_credentials::Migration),
          Box::new(m20260217_000002_mcp_url_rename::Migration),
          Box::new(m20260218_000001_fix_mcp_schema::Migration),
          Box::new(m20260227_000001_drop_legacy_user_role::Migration),
          Box::new(m20260227_000002_add_permission_description_key::Migration),
         ]
    }
}
