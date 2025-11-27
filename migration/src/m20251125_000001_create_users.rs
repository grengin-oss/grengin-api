use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
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
                        ColumnDef::new(Users::Picture)
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
                        ColumnDef::new(Users::Password)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::GoogleId)
                            .text()
                            .null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Users::AzureId)
                            .text()
                            .null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(Users::MfaEnabled)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::MfaSecret)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::LastLoginAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::PasswordChangedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::Role)
                            .string() // backing type of UserRole enum (rs_type = "String")
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Users::Hd)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Users::Department)
                            .string()
                            .null(),
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
                    .name("idx-users-id")
                    .table(Users::Table)
                    .col(Users::Id)
                    .to_owned(),
            )
            .await?;

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
                    .name("idx-users-azure-id")
                    .table(Users::Table)
                    .col(Users::AzureId)
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
    #[sea_orm(iden = "picture")]
    Picture,
    #[sea_orm(iden = "email")]
    Email,
    #[sea_orm(iden = "emailVerified")]
    EmailVerified,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "password")]
    Password,
    #[sea_orm(iden = "googleId")]
    GoogleId,
    #[sea_orm(iden = "azureId")]
    AzureId,
    #[sea_orm(iden = "mfaEnabled")]
    MfaEnabled,
    #[sea_orm(iden = "mfaSecret")]
    MfaSecret,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
    #[sea_orm(iden = "lastLoginAt")]
    LastLoginAt,
    #[sea_orm(iden = "passwordChangedAt")]
    PasswordChangedAt,
    #[sea_orm(iden = "role")]
    Role,
    #[sea_orm(iden = "hd")]
    Hd,
    #[sea_orm(iden = "department")]
    Department,
    #[sea_orm(iden = "metadata")]
    Metadata,
}
