// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use sea_orm::{ActiveModelTrait, EntityTrait, IntoActiveModel};
use uuid::Uuid;

use crate::{
    auth::error::AuthError, dto::branding::Branding, models::branding, state::SharedState,
};

pub fn create_default_branding() -> branding::Model {
    branding::Model {
        id: Uuid::new_v4(),
        name: "Grengin".into(),
        logo_url: None,
        color_primary: "#4079c5".into(),
        color_accent: "#2d906b".into(),
        font_family: "Coustard".into(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

pub fn model_to_response(model: &branding::Model) -> Branding {
    Branding {
        name: model.name.clone(),
        logo_url: model.logo_url.clone(),
        color_primary: model.color_primary.clone(),
        color_accent: model.color_accent.clone(),
        font_family: model.font_family.clone(),
    }
}

pub async fn get_or_create_branding(app_state: &SharedState) -> Result<branding::Model, AuthError> {
    let branding_model = branding::Entity::find()
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("branding query error: {e}");
            AuthError::DbTimeout
        })?;

    if let Some(model) = branding_model {
        Ok(model)
    } else {
        let default_branding = create_default_branding();
        default_branding
            .clone()
            .into_active_model()
            .insert(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("branding insert error: {e}");
                AuthError::DbTimeout
            })?;
        Ok(default_branding)
    }
}
