pub use sea_orm_migration::prelude::*;

mod m20251125_000001_create_users;
mod m20251125_000002_create_oauth_sessions;
mod m20251125_000003_create_conversations;
mod m20251125_000004_create_messages;
mod m20251125_000005_create_prompt_templates;
mod m20250201_000001_make_previous_message_id_nullable;
mod m20250201_000002_add_request_id_to_messages;
mod m20251211_000005_add_deleted_to_messages;

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
          Box::new(m20251211_000005_add_deleted_to_messages::Migration)
          ]
    }
}
