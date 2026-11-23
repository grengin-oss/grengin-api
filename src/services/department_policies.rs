// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

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
    let mut visited = HashSet::new();
    let mut effective: Option<Vec<DepartmentModelKey>> = None;
    while let Some(dept_id) = current {
        if !visited.insert(dept_id) || visited.len() > 10 {
            return Err(sea_orm::DbErr::Custom("invalid department ancestry".into()));
        }

        let explicit = load_allowed_models(db, dept_id).await?;
        if !explicit.is_empty() {
            match effective.as_mut() {
                None => effective = Some(explicit),
                Some(models) => {
                    let allowed: HashSet<_> = explicit
                        .iter()
                        .map(|model| model_policy_key(&model.provider, &model.model))
                        .collect();
                    models.retain(|model| {
                        allowed.contains(&model_policy_key(&model.provider, &model.model))
                    });
                }
            }
        }

        let parent = load_parent_row(db, dept_id).await?;
        current = parent.parent_id;
    }
    Ok(effective)
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
        .map(|m| model_policy_key(&m.provider, &m.model))
        .collect();
    for model in requested {
        let key = model_policy_key(&model.provider, &model.model);
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

/// Department limits follow the model across direct and workspace funding.
pub fn model_policy_key(provider: &str, model: &str) -> (String, String) {
    let (provider, model) = if provider.eq_ignore_ascii_case("grengin") {
        super::grengin_provider::split_model(model).unwrap_or((provider, model))
    } else {
        (provider, model)
    };
    (provider.to_lowercase(), model.to_lowercase())
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
    let key = model_policy_key(provider, model);
    Ok(allowed
        .iter()
        .any(|m| model_policy_key(&m.provider, &m.model) == key))
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

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, Database};

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
    async fn ancestors_constrain_children_after_policy_changes_and_moves() {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").unwrap();
        assert!(url.ends_with("/grengin_security_test"));
        let db = Database::connect(url).await.unwrap();
        let (root, parent, child, other) = (
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::new_v4(),
        );
        for (id, parent_id) in [
            (root, None),
            (parent, Some(root)),
            (child, Some(parent)),
            (other, None),
        ] {
            let parent_value = parent_id
                .map(|id| format!("'{id}'"))
                .unwrap_or("NULL".into());
            db.execute_unprepared(&format!(r#"INSERT INTO departments (id, name, path, depth, "parentId", "createdAt", "updatedAt") VALUES ('{id}', 'policy-test', 'd{}', 0, {parent_value}, now(), now())"#, id.simple())).await.unwrap();
        }
        assert!(
            effective_allowed_models(&db, child)
                .await
                .unwrap()
                .is_none()
        );
        for (department, provider, model) in [
            (root, "openai", "a"),
            (root, "openai", "b"),
            (child, "grengin", "openai/a"),
            (child, "openai", "b"),
            (other, "openai", "c"),
        ] {
            db.execute_unprepared(&format!(r#"INSERT INTO department_allowed_models (id, "departmentId", provider, model, "createdAt", "updatedAt") VALUES ('{}', '{department}', '{provider}', '{model}', now(), now())"#, Uuid::new_v4())).await.unwrap();
        }
        assert!(
            check_model_allowed(&db, Some(child), "openai", "a")
                .await
                .unwrap()
        );
        db.execute_unprepared(&format!(r#"DELETE FROM department_allowed_models WHERE "departmentId" = '{root}' AND model = 'a'"#)).await.unwrap();
        assert!(
            !check_model_allowed(&db, Some(child), "grengin", "openai/a")
                .await
                .unwrap()
        );
        assert!(
            check_model_allowed(&db, Some(child), "openai", "b")
                .await
                .unwrap()
        );
        db.execute_unprepared(&format!(
            r#"UPDATE departments SET "parentId" = '{other}' WHERE id = '{parent}'"#
        ))
        .await
        .unwrap();
        assert!(
            effective_allowed_models(&db, child)
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(
            !check_model_allowed(&db, Some(child), "openai", "b")
                .await
                .unwrap()
        );
        db.execute_unprepared(&format!(
            r#"UPDATE departments SET "parentId" = '{child}' WHERE id = '{other}'"#
        ))
        .await
        .unwrap();
        assert!(
            effective_allowed_models(&db, child).await.is_err(),
            "a corrupt cycle must not become unrestricted access"
        );
        db.execute_unprepared(&format!(r#"UPDATE departments SET "parentId" = NULL WHERE id IN ('{root}', '{parent}', '{child}', '{other}'); DELETE FROM department_allowed_models WHERE "departmentId" IN ('{root}', '{parent}', '{child}', '{other}'); DELETE FROM departments WHERE id IN ('{root}', '{parent}', '{child}', '{other}')"#)).await.unwrap();
    }
}
