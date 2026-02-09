use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdministeredDepartmentsResponse {
    pub administered_departments: Vec<Uuid>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePermissionsResponse {
    #[schema(value_type = Object)]
    pub permissions: serde_json::Value,
    #[schema(value_type = Object)]
    pub mcp_access: serde_json::Value,
    pub administered_departments: Vec<String>,
}
