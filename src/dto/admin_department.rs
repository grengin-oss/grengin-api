use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize,ToSchema)]
pub struct Department {
    pub name:String,
    pub user_count:i64,
}

#[derive(Serialize,ToSchema)]
pub struct DepartmentResponse {
     pub departments:Vec<Department>,
}