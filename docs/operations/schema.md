# Session database schema

dux stores session metadata in `sessions.sqlite3`. `PRAGMA user_version` is
currently `5`; startup applies each numbered migration in one transaction with
its version bump.

## `agent_sessions`

Migration `0005_shared_workspace.sql` rebuilds the table and retains every
pre-v5 column. It adds:

- `shared_workspace INTEGER NOT NULL DEFAULT 0` — dark in Phase 1; no runtime
  subsystem changes behavior based on it yet.
- `agent_handle TEXT NOT NULL UNIQUE` — immutable local session identity,
  limited to 1–64 lowercase ASCII letters, digits, `_`, and `-`.
- `deleted_at TEXT` — nullable RFC 3339 tombstone timestamp.

Existing handles are derived from the worktree-path basename in primary-key
order. Collisions receive deterministic `-2`, `-3`, … suffixes. The rebuild,
Rust backfill, `session_prs` copy, index recreation, `foreign_key_check`, and
`user_version = 5` commit or roll back together.

Normal session loads return only rows where `deleted_at IS NULL`. UI deletion
sets the tombstone and retains the complete row. Destructive maintenance can
load tombstones explicitly and physically remove a row only after its purge
work succeeds.

## `session_prs`

`session_prs(session_id)` references `agent_sessions(id) ON DELETE CASCADE`.
Migration 0005 rebuilds this table inside the same transaction so PR rows and
the foreign key survive the parent-table rebuild.
