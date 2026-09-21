// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::path::{Component, Path};

pub fn safe_file_name(value: &str) -> Option<&str> {
    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !value.is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::safe_file_name;

    #[test]
    fn accepts_leaf_names_only() {
        assert_eq!(safe_file_name("notes.txt"), Some("notes.txt"));
        assert_eq!(safe_file_name(".notes"), Some(".notes"));

        for invalid in [
            "",
            ".",
            "..",
            "../notes.txt",
            "dir/notes.txt",
            "/etc/passwd",
        ] {
            assert_eq!(safe_file_name(invalid), None, "{invalid} must fail");
        }
    }
}
