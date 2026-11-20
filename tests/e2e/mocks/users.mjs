// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Test identities known to the mock IdP. `owner` matches BOOTSTRAP_SUPER_ADMIN_EMAIL, so its
// first login links to the bootstrapped Super Admin.
export const USERS = {
  owner: { sub: 'e2e-owner', email: 'owner@e2e.grengin.test', name: 'E2E Owner' },
  member: { sub: 'e2e-member', email: 'member@e2e.grengin.test', name: 'E2E Member' },
  deptadmin: { sub: 'e2e-deptadmin', email: 'deptadmin@e2e.grengin.test', name: 'E2E Department Admin' },
};
