# Session database schema

dux stores session metadata in `sessions.sqlite3`. There is no numbered
migration ledger: `SessionStore::migrate()` in `crates/dux-core/src/storage.rs`
runs on every open, creating tables with `create table if not exists` and adding
newer columns with the idempotent `ensure_column` helper. See
[the schema policy](../contributing/schema-policy.md) for the rules and
`crates/dux-core/tests/upgrade_database.rs` for the upgrade tests.

The sections below describe the fork's additions to that schema. Each lands as
its own appended block in `migrate()`.

## `agent_sessions`

<!-- INTEGRATION: path pending evergreen (shared-workspace columns in crates/dux-core/src/storage.rs) -->

The shared-workspace block adds:

- `shared_workspace INTEGER NOT NULL DEFAULT 0`: durable lifecycle mode.
  Shared rows use the registered checkout and never own a worktree or branch.
- `agent_handle TEXT`: immutable local session identity, limited to 1 to 64
  lowercase ASCII letters, digits, `_`, and `-`, unique per store.
- `deleted_at TEXT`: nullable RFC 3339 tombstone timestamp.

Existing handles are derived from the worktree-path basename in primary-key
order. Collisions receive deterministic `-2`, `-3`, ... suffixes. Uniqueness and
the handle format need constraints `ALTER TABLE ADD COLUMN` cannot express, so
this step rebuilds the table in one transaction (rebuild, Rust backfill,
`session_prs` copy, index recreation, `foreign_key_check`) and detects on later
opens that it already ran.

<!-- INTEGRATION: path pending palmtree (provider_session_ids column) -->

The resume block adds `provider_session_ids TEXT NOT NULL DEFAULT '{}'`. The
JSON object maps a provider name to that agent's exact provider conversation
UUID. SQLite remains the sole durable authority. Startup recovery and
fresh-launch capture update this column, and shared sessions never substitute a
latest/recency selector for a missing or invalid UUID.

Normal session loads return only rows where `deleted_at IS NULL`. UI deletion
sets the tombstone and retains the complete row. Destructive maintenance can
load tombstones explicitly and physically remove a row only after its purge
work succeeds.

## `session_prs`

`session_prs(session_id)` references `agent_sessions(id) ON DELETE CASCADE`.
The shared-workspace table rebuild copies this table inside the same
transaction so PR rows and the foreign key survive the parent-table rebuild.

## AMQ ownership metadata

<!-- INTEGRATION: path pending maple (AMQ ownership records) -->

Each DUX_HOME has a stable UUID in `store-id`. Creation is serialized by
`.store-id.lock`. The UUID is written and synced once, then reused across
restarts.

Under a configured shared AMQ root, `agents/<agent_handle>/.dux-amq-source`
is an atomic JSON ownership record containing `store_id`, `session_id`, and an
optional legacy `wake_pid` left by pre-managed-wake installs. New wrappers let
AMQ bind wake to the provider process and record its lifecycle in `.wake.lock`
instead. Before relaunch they call AMQ's guarded `wake recover-owner`, which
removes a dead exact-owner claim but refuses to steal one from a live owner.
Rust and all provider wrappers hold `meta/config.lock` with `flock` for the
complete owner/config read-modify-write. If that mandatory lock cannot be
acquired, registration fails closed.

Pre-existing path/symlink markers are upgraded only when the path maps to
exactly one row in the current store. Ambiguous paths, malformed markers,
ownerless agent directories, and markers owned by another store are preserved
as foreign. A backfill or new session that encounters one receives the next
available `-2`, `-3`, ... handle under the shared lock. After that reservation,
the handle is immutable.

Ordinary deletion performs AMQ cleanup only for an exact owner match. Foreign,
legacy, missing, or unreadable markers are left untouched and never block the
local session tombstone. Exact-owner cleanup removes the handle from AMQ's live
`config.json`, clears and then terminates any legacy recorded wake PID, and
keeps the inbox plus owner record reserved. Owner-bound wake exits with its
provider. The hard-purge cascade targets the persisted `agent_handle` rather
than a worktree basename.
