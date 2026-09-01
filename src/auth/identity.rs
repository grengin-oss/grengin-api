// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub display_name: Option<String>,
    pub picture: Option<String>,
    pub hosted_domain: Option<String>,
}
