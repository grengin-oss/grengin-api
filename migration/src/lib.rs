pub use sea_orm_migration::prelude::*;

mod m20251125_000001_create_users;
mod m20251125_000002_create_oauth_sessions;
mod m20251125_000003_create_conversations;
mod m20251125_000004_create_messages;
mod m20251125_000005_create_prompt_templates;

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
          ]
    }
}
