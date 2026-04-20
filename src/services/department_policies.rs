use std::collections::{HashMap, HashSet};

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::{
    dto::admin_department::DepartmentModelKey,
    models::{department_allowed_models, departments},
};

#[derive(Debug)]
struct DepartmentParentRow {
    parent_id: Option<Uuid>,
    retention_days: Option<i32>,
}

pub async fn load_allowed_models_map(
    db: &sea_orm::DatabaseConnection,
    department_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<DepartmentModelKey>>, sea_orm::DbErr> {
    if department_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = department_allowed_models::Entity::find()
        .filter(department_allowed_models::Column::DepartmentId.is_in(department_ids.to_vec()))
        .order_by_asc(department_allowed_models::Column::Provider)
        .order_by_asc(department_allowed_models::Column::Model)
        .all(db)
        .await?;

    let mut map: HashMap<Uuid, Vec<DepartmentModelKey>> = HashMap::new();
    for row in rows {
        map.entry(row.department_id)
            .or_default()
            .push(DepartmentModelKey {
                provider: row.provider,
                model: row.model,
            });
    }
    Ok(map)
}

pub async fn load_allowed_models(
    db: &sea_orm::DatabaseConnection,
    department_id: Uuid,
) -> Result<Vec<DepartmentModelKey>, sea_orm::DbErr> {
    let rows = department_allowed_models::Entity::find()
        .filter(department_allowed_models::Column::DepartmentId.eq(department_id))
        .order_by_asc(department_allowed_models::Column::Provider)
        .order_by_asc(department_allowed_models::Column::Model)
        .all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| DepartmentModelKey {
            provider: row.provider,
            model: row.model,
        })
        .collect())
}

pub async fn effective_allowed_models(
    db: &sea_orm::DatabaseConnection,
    department_id: Uuid,
) -> Result<Option<Vec<DepartmentModelKey>>, sea_orm::DbErr> {
    let mut current = Some(department_id);
    let mut depth = 0;
    while let Some(dept_id) = current {
        depth += 1;
        if depth > 10 {
            break;
        }

        let explicit = load_allowed_models(db, dept_id).await?;
        if !explicit.is_empty() {
            return Ok(Some(explicit));
        }

        let parent = load_parent_row(db, dept_id).await?;
        current = parent.parent_id;
    }
    Ok(None)
}

pub async fn validate_allowed_models_subset(
    db: &sea_orm::DatabaseConnection,
    department_id: Option<Uuid>,
    requested: &[DepartmentModelKey],
) -> Result<(), String> {
    let Some(parent_id) = department_id else {
        return Ok(());
    };
    let parent_effective = effective_allowed_models(db, parent_id)
        .await
        .map_err(|e| e.to_string())?;
    let Some(parent_allowed) = parent_effective else {
        return Ok(());
    };
    let parent_set: HashSet<(String, String)> = parent_allowed
        .into_iter()
        .map(|m| (m.provider.to_lowercase(), m.model.to_lowercase()))
        .collect();
    for model in requested {
        let key = (model.provider.to_lowercase(), model.model.to_lowercase());
        if !parent_set.contains(&key) {
            return Err(format!(
                "model {}:{} is not allowed by parent policy",
                model.provider, model.model
            ));
        }
    }
    Ok(())
}

pub async fn validate_retention_days(
    db: &sea_orm::DatabaseConnection,
    parent_id: Option<Uuid>,
    retention_days: Option<i32>,
) -> Result<(), String> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    let parent = load_parent_row(db, parent_id)
        .await
        .map_err(|e| e.to_string())?;
    if let (Some(parent_days), Some(child_days)) = (parent.retention_days, retention_days) {
        if child_days > parent_days {
            return Err("retention_days must be <= parent retention_days".to_string());
        }
    }
    Ok(())
}

pub async fn check_model_allowed(
    db: &sea_orm::DatabaseConnection,
    department_id: Option<Uuid>,
    provider: &str,
    model: &str,
) -> Result<bool, sea_orm::DbErr> {
    let Some(dept_id) = department_id else {
        return Ok(true);
    };
    let effective = effective_allowed_models(db, dept_id).await?;
    let Some(allowed) = effective else {
        return Ok(true);
    };
    let provider = provider.to_lowercase();
    let model = model.to_lowercase();
    Ok(allowed
        .iter()
        .any(|m| m.provider.to_lowercase() == provider && m.model.to_lowercase() == model))
}

async fn load_parent_row(
    db: &sea_orm::DatabaseConnection,
    department_id: Uuid,
) -> Result<DepartmentParentRow, sea_orm::DbErr> {
    let row = departments::Entity::find_by_id(department_id)
        .select_only()
        .column(departments::Column::ParentId)
        .column(departments::Column::RetentionDays)
        .into_tuple::<(Option<Uuid>, Option<i32>)>()
        .one(db)
        .await?;
    if let Some((parent_id, retention_days)) = row {
        Ok(DepartmentParentRow {
            parent_id,
            retention_days,
        })
    } else {
        Ok(DepartmentParentRow {
            parent_id: None,
            retention_days: None,
        })
    }
}
