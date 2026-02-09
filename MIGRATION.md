# Authorization Layer Migration

## Overview
This update adds permission-based authorization (RBAC), scoped role assignments, MCP access rules, and an `effective_permissions` cache on users. It keeps legacy `users.role` checks available as a fallback when no role assignments exist.

## Apply
1. Run migrations:
   ```sh
   cargo run -p migration -- up
   ```
2. (Optional) Restart the API so the new routes are available.

## Seeds & Data Migration
The migration seeds:
- Permission catalog (`permissions`)
- System roles (`roles`) and role-permission mappings (`role_permissions`)

Legacy user roles are mapped on migration:
- `admin` and `superadmin` -> `Super Admin` role (org-wide)
- `observer` -> `Observer` role (org-wide)
- `user` -> no role assignment

## Rollback
To rollback the latest migration:
```sh
cargo run -p migration -- down
```
This drops the RBAC and MCP tables and removes the new user columns.

## Notes / Deltas
- `user_role_assignments` uses a surrogate `id` primary key with a unique constraint on `(userId, roleId, scopeDepartmentId)` because scope is nullable.
- Department Admin assignments are required to be scoped (no org-wide assignment).
- For list endpoints without an explicit target department, scoped permissions require org-wide assignments (to avoid over-broad data exposure).
- Permission catalog / role permissions are derived from the provided spec; if the spec’s action list differs, adjust seeds and role mappings accordingly.
- A new `auth_audit_events` table captures `auth.*` audit events with JSON payloads.
