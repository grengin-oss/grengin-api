use serde::Deserialize;

#[derive(Deserialize)]
pub struct PaginationQuery {
   pub limit:Option<u64>,
   pub offset:Option<u64>,
   pub search:Option<String>,
   pub archived:Option<bool>,
}