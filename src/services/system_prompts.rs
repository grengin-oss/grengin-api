use std::collections::HashMap;

use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;

use crate::{
    dto::prompts::PromptSource,
    models::{
        department_prompt_assignments, departments, role_prompts, roles, user_prompt_preferences,
        user_role_assignments, users,
    },
};

pub struct ResolvedPrompt {
    pub prompt_id: Option<Uuid>,
    pub prompt_text: String,
    pub source: PromptSource,
    pub variables: Option<Vec<String>>,
}

pub async fn resolve_system_prompt(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<Option<ResolvedPrompt>, sea_orm::DbErr> {
    let user = users::Entity::find_by_id(user_id).one(db).await?;
    let Some(user) = user else {
        return Ok(None);
    };

    if let Some(resolved) = resolve_user_preference(db, &user).await? {
        return Ok(Some(resolved));
    }

    if let Some(resolved) = resolve_department_prompt(db, &user).await? {
        return Ok(Some(resolved));
    }

    if let Some(resolved) = resolve_system_default(db, &user).await? {
        return Ok(Some(resolved));
    }

    Ok(Some(ResolvedPrompt {
        prompt_id: None,
        prompt_text: String::new(),
        source: PromptSource::None,
        variables: None,
    }))
}

async fn resolve_user_preference(
    db: &sea_orm::DatabaseConnection,
    user: &users::Model,
) -> Result<Option<ResolvedPrompt>, sea_orm::DbErr> {
    let preference = user_prompt_preferences::Entity::find()
        .filter(user_prompt_preferences::Column::UserId.eq(user.id))
        .filter(user_prompt_preferences::Column::IsActive.eq(true))
        .one(db)
        .await?;

    let Some(preference) = preference else {
        return Ok(None);
    };

    if let Some(custom_prompt) = preference.custom_prompt_text.clone() {
        let rendered = render_prompt(db, user, &custom_prompt).await?;
        return Ok(Some(ResolvedPrompt {
            prompt_id: None,
            prompt_text: rendered,
            source: PromptSource::UserCustom,
            variables: None,
        }));
    }

    if let Some(prompt_id) = preference.prompt_id {
        if let Some(prompt) = role_prompts::Entity::find_by_id(prompt_id).one(db).await? {
            let rendered = render_prompt(db, user, &prompt.prompt_text).await?;
            let variables = parse_variables(&prompt.variables);
            increment_usage(db, prompt.id).await?;
            return Ok(Some(ResolvedPrompt {
                prompt_id: Some(prompt.id),
                prompt_text: rendered,
                source: PromptSource::UserPrompt,
                variables,
            }));
        }
    }

    Ok(None)
}

async fn resolve_department_prompt(
    db: &sea_orm::DatabaseConnection,
    user: &users::Model,
) -> Result<Option<ResolvedPrompt>, sea_orm::DbErr> {
    let Some(department_id) = user.department_id else {
        return Ok(None);
    };

    let assignment = department_prompt_assignments::Entity::find()
        .filter(department_prompt_assignments::Column::DepartmentId.eq(department_id))
        .order_by_asc(department_prompt_assignments::Column::Priority)
        .one(db)
        .await?;

    let Some(assignment) = assignment else {
        return Ok(None);
    };

    if let Some(prompt) = role_prompts::Entity::find_by_id(assignment.prompt_id)
        .one(db)
        .await?
    {
        let rendered = render_prompt(db, user, &prompt.prompt_text).await?;
        let variables = parse_variables(&prompt.variables);
        increment_usage(db, prompt.id).await?;
        return Ok(Some(ResolvedPrompt {
            prompt_id: Some(prompt.id),
            prompt_text: rendered,
            source: PromptSource::Department,
            variables,
        }));
    }

    Ok(None)
}

async fn resolve_system_default(
    db: &sea_orm::DatabaseConnection,
    user: &users::Model,
) -> Result<Option<ResolvedPrompt>, sea_orm::DbErr> {
    let org_role_ids: Vec<Uuid> = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::RoleId)
        .filter(user_role_assignments::Column::UserId.eq(user.id))
        .filter(user_role_assignments::Column::ScopeDepartmentId.is_null())
        .into_tuple::<Uuid>()
        .all(db)
        .await?;

    let mut prompt = if org_role_ids.is_empty() {
        None
    } else {
        role_prompts::Entity::find()
            .filter(role_prompts::Column::IsSystem.eq(true))
            .filter(role_prompts::Column::RoleId.is_in(org_role_ids))
            .order_by_asc(role_prompts::Column::CreatedAt)
            .one(db)
            .await?
    };

    if prompt.is_none() {
        let user_role_id = roles::Entity::find()
            .select_only()
            .column(roles::Column::Id)
            .filter(roles::Column::Name.eq("User"))
            .into_tuple::<Uuid>()
            .one(db)
            .await?;
        if let Some(user_role_id) = user_role_id {
            prompt = role_prompts::Entity::find()
                .filter(role_prompts::Column::IsSystem.eq(true))
                .filter(role_prompts::Column::RoleId.eq(user_role_id))
                .order_by_asc(role_prompts::Column::CreatedAt)
                .one(db)
                .await?;
        }
    }

    if prompt.is_none() {
        prompt = role_prompts::Entity::find()
            .filter(role_prompts::Column::IsSystem.eq(true))
            .order_by_asc(role_prompts::Column::CreatedAt)
            .one(db)
            .await?;
    }

    let Some(prompt) = prompt else {
        return Ok(None);
    };

    let rendered = render_prompt(db, user, &prompt.prompt_text).await?;
    let variables = parse_variables(&prompt.variables);
    increment_usage(db, prompt.id).await?;
    Ok(Some(ResolvedPrompt {
        prompt_id: Some(prompt.id),
        prompt_text: rendered,
        source: PromptSource::SystemDefault,
        variables,
    }))
}

async fn render_prompt(
    db: &sea_orm::DatabaseConnection,
    user: &users::Model,
    text: &str,
) -> Result<String, sea_orm::DbErr> {
    let user_name = user.name.clone().unwrap_or_else(|| "User".to_string());
    let department_name = if let Some(department_id) = user.department_id {
        let name = departments::Entity::find_by_id(department_id)
            .select_only()
            .column(departments::Column::Name)
            .into_tuple::<String>()
            .one(db)
            .await?;
        name.unwrap_or_else(|| "Department".to_string())
    } else {
        "Department".to_string()
    };
    let company_name = std::env::var("COMPANY_NAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "Company".to_string());

    let mut replacements = HashMap::new();
    replacements.insert("{{user_name}}", user_name);
    replacements.insert("{{department}}", department_name);
    replacements.insert("{{company_name}}", company_name);

    Ok(apply_replacements(text, &replacements))
}

fn apply_replacements(text: &str, replacements: &HashMap<&str, String>) -> String {
    let mut out = text.to_string();
    for (key, value) in replacements {
        out = out.replace(key, value);
    }
    out
}

fn parse_variables(value: &Option<serde_json::Value>) -> Option<Vec<String>> {
    let Some(value) = value else {
        return None;
    };
    let Some(array) = value.as_array() else {
        return None;
    };
    let mut vars = Vec::new();
    for item in array {
        if let Some(val) = item.as_str() {
            vars.push(val.to_string());
        }
    }
    if vars.is_empty() {
        None
    } else {
        Some(vars)
    }
}

async fn increment_usage(
    db: &sea_orm::DatabaseConnection,
    prompt_id: Uuid,
) -> Result<(), sea_orm::DbErr> {
    role_prompts::Entity::update_many()
        .col_expr(
            role_prompts::Column::UsageCount,
            Expr::col(role_prompts::Column::UsageCount).add(1),
        )
        .filter(role_prompts::Column::Id.eq(prompt_id))
        .exec(db)
        .await?;
    Ok(())
}
