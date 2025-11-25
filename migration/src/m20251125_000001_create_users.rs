use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // users table
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Users::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Users::Status)
                            .string() // backing type of UserStatus enum (rs_type = "String")
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::Avatar)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::Email)
                            .text()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Users::EmailVerified)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::Name)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::GoogleId)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::TwoFactorAuth)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::TwoFactorSecret)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::AzureId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::CreatedOn)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::UpdatedOn)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::LastLoginOn)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::Metadata)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Indexes for fields marked `indexed` in the model
        manager
            .create_index(
                Index::create()
                    .name("idx-users-email")
                    .table(Users::Table)
                    .col(Users::Email)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-users-google-id")
                    .table(Users::Table)
                    .col(Users::GoogleId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-users-two-factor-auth")
                    .table(Users::Table)
                    .col(Users::TwoFactorAuth)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Users::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,

    // camelCase column names because of `rename_all = "camelCase"`
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "status")]
    Status,
    #[sea_orm(iden = "avatar")]
    Avatar,
    #[sea_orm(iden = "email")]
    Email,
    #[sea_orm(iden = "emailVerified")]
    EmailVerified,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "googleId")]
    GoogleId,
    #[sea_orm(iden = "twoFactorAuth")]
    TwoFactorAuth,
    #[sea_orm(iden = "twoFactorSecret")]
    TwoFactorSecret,
    #[sea_orm(iden = "azureId")]
    AzureId,
    #[sea_orm(iden = "createdOn")]
    CreatedOn,
    #[sea_orm(iden = "updatedOn")]
    UpdatedOn,
    #[sea_orm(iden = "lastLoginOn")]
    LastLoginOn,
    #[sea_orm(iden = "metadata")]
    Metadata,
}
