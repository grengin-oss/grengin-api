use uuid::Uuid;

pub fn ltree_label_from_uuid(id: Uuid) -> String {
    id.simple().to_string()
}
