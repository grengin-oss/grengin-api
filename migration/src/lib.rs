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
         ]
    }
}
