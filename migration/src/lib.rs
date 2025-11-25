pub use sea_orm_migration::prelude::*;

mod m20251125_000001_create_users;
mod m20251125_000002_create_oauth_sessions;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(m20251125_000001_create_users::Migration),Box::new(m20251125_000002_create_oauth_sessions::Migration)]
    }
}
