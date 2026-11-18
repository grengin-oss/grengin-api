// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

pub mod azure;
pub mod claims;
pub mod encryption;
pub mod error;
pub mod google;
pub mod identity;
pub mod jwt;
pub mod oauth2_identity;
pub mod permissions;
pub mod provider_config;
pub mod sso_provider;
pub mod sso_proxy;

#[cfg(test)]
mod tests;
