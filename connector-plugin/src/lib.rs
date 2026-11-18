// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

mod manifest;
mod oauth;
mod response;
mod runtime;
mod security;
mod session_cache;

pub use manifest::*;
pub use oauth::{InvalidOAuthExpiry, OAuthExpirySource};
pub use runtime::{CallContext, CompiledConnector, ConnectorError};
pub use security::validate_url as validate_endpoint_url;
pub use session_cache::{SessionCache, SessionCacheError, SessionCachePolicy, SessionLease};

pub fn runtime_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
