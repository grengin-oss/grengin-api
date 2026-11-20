// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

pub const MAX_PAGE_LIMIT: u64 = 100;

/// The page size for a listing request: `default` when absent or zero, never above
/// `MAX_PAGE_LIMIT`, so a request can neither divide by zero nor load a whole table.
pub fn page_limit(requested: Option<u64>, default: u64) -> u64 {
    requested
        .filter(|limit| *limit > 0)
        .unwrap_or(default)
        .clamp(1, MAX_PAGE_LIMIT)
}

#[cfg(test)]
mod tests {
    use super::{MAX_PAGE_LIMIT, page_limit};

    #[test]
    fn a_page_limit_is_never_zero_and_never_unbounded() {
        assert_eq!(page_limit(None, 30), 30);
        assert_eq!(page_limit(Some(0), 30), 30);
        assert_eq!(page_limit(Some(7), 30), 7);
        assert_eq!(page_limit(Some(1_000_000), 30), MAX_PAGE_LIMIT);
        assert_eq!(page_limit(None, 0), 1);
    }
}
