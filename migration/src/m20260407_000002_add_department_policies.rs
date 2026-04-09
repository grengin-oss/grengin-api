use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,
    #[sea_orm(iden = "retentionDays")]
    RetentionDays,
}

#[derive(DeriveIden)]
enum DepartmentAllowedModels {
    #[sea_orm(iden = "department_allowed_models")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "departmentId")]
    DepartmentId,
    #[sea_orm(iden = "provider")]
    Provider,
    #[sea_orm(iden = "model")]
    Model,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum DepartmentsRef {
    #[sea_orm(iden = "departments")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Departments::Table)
                    .add_column(ColumnDef::new(Departments::RetentionDays).integer().null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(DepartmentAllowedModels::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(DepartmentAllowedModels::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(DepartmentAllowedModels::DepartmentId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(DepartmentAllowedModels::Provider).string().not_null())
                    .col(ColumnDef::new(DepartmentAllowedModels::Model).string().not_null())
                    .col(
                        ColumnDef::new(DepartmentAllowedModels::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(DepartmentAllowedModels::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .from(
                        DepartmentAllowedModels::Table,
                        DepartmentAllowedModels::DepartmentId,
                    )
                    .to(DepartmentsRef::Table, DepartmentsRef::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uniq_department_allowed_model")
                    .table(DepartmentAllowedModels::Table)
                    .col(DepartmentAllowedModels::DepartmentId)
                    .col(DepartmentAllowedModels::Provider)
                    .col(DepartmentAllowedModels::Model)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_department_allowed_models_department")
                    .table(DepartmentAllowedModels::Table)
                    .col(DepartmentAllowedModels::DepartmentId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(DepartmentAllowedModels::Table).to_owned())
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Departments::Table)
                    .drop_column(Departments::RetentionDays)
                    .to_owned(),
            )
            .await
    }
}
