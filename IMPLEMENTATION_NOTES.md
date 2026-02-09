# Implementation Notes (Authorization Layer)

## Repo scan summary
- Stack: Rust (Axum), SeaORM + SeaORM migrations in `migration/`.
- Authn: JWT claims include `user_id` + legacy `UserRole` enum; handlers gate with `match claims.role`.
- Departments: hierarchical `departments` table with Postgres `ltree` path + depth.
- No existing RBAC tables, MCP server models, or audit event system.

## Proposed approach (minimal disruption)
- Add RBAC tables + columns via new migrations with reversible `down()`.
- Implement a centralized authorization service that:
  - Evaluates new permission assignments first.
  - Falls back to legacy `UserRole` checks when no assignments exist (compat layer).
- Reuse `departments.path` for scoped permission checks (ltree prefix).
- Add an `effective_permissions` cache on `users` and recompute via service hooks where role assignments or department hierarchy change.
- Introduce minimal audit event storage (new table) aligned with spec event names.
- Add new endpoints under existing admin router and secure existing ones where mappings are clear.

## Open deltas
- MCP server model and rule storage will be new (no existing equivalent found).
- Audit payload schema is not defined in repo; will implement a generic `auth_audit_events` table with JSON payloads and document in `MIGRATION.md`.
