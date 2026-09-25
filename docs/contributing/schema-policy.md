# Schema policy

dux persists user data in two places whose shape changes over time:

1. The SQLite database `sessions.sqlite3`, created and upgraded by
   `SessionStore::migrate()` in `crates/dux-core/src/storage.rs`.
2. The TOML file `config.toml`, parsed by `crates/dux-core/src/config.rs`
   and upgraded by the load-time migrations in
   `crates/dux-core/src/config_migrate.rs`.

Both follow the same contract: data written by any older dux build must keep
loading on a newer one, with nothing lost, and every upgrade path is backed by
a test.

## SQLite schema

### How upgrades work

There are no numbered migration files and no `PRAGMA user_version` ledger.
`migrate()` runs on every `SessionStore::open` and is idempotent:

- `create table if not exists ...` declares each table in its current shape,
  so a fresh database is created complete.
- `ensure_column(conn, table, column, decl)` adds a column only when it is
  missing, and returns whether it did. An older database therefore gains each
  new column the first time a newer dux opens it.
- One-time backfills (for example `title`, `initial_branch`, `sort_order`) run
  keyed off the `true` that `ensure_column` returns, or are written to be safe
  to repeat, so a second open never rewrites data.

Because `open` runs at every startup and on background persistence, every step
must be safe to run any number of times.

### Adding a column

- Add it to the `create table if not exists` statement AND append an
  `ensure_column` call for it in `migrate()`. The first covers fresh databases,
  the second covers upgrades.
- Always nullable or `DEFAULT`-ed. An older dux binary's `INSERT` names none of
  the new columns and must still satisfy the schema after a downgrade.
- If the default encodes a safety decision, choose the value that is safe for
  pre-existing rows whose true state is unknowable (see `branch_provenance`,
  which defaults to `'created'`).
- Pick a fresh name. Never reuse the name of a dropped column, because backups
  taken before the drop may still carry the old data.

### One block per workstream

Several workstreams extend the schema in parallel (shared workspace, resume,
peer messaging, and others). Each appends its own clearly commented block of
`ensure_column` calls and backfills at the end of `migrate()`, rather than
interleaving edits into existing blocks. This keeps merges mechanical and makes
the provenance of each column obvious. Never reorder or edit another block.

### Changes `ensure_column` cannot express

`ALTER TABLE ADD COLUMN` cannot add `NOT NULL` without a default, `UNIQUE`, or
`CHECK` constraints. When a change needs one of those, rebuild the table inside
a single transaction (create new, copy, drop old, rename, recreate indexes, run
`foreign_key_check`) and make the rebuild detect whether it already happened so
it stays idempotent. Rebuilding a table referenced by a foreign key (such as
`session_prs`) must carry the child rows through the same transaction.

### Renaming or dropping

Don't rename. Introduce a new column, copy data inside `migrate()`, deprecate
the old one in code, and drop it only after one full release in which a
deprecation log line fired and the release notes announced the drop.

### Required test

Every schema change must be covered in `crates/dux-core/tests/upgrade_database.rs`,
which opens databases transcribed from real older releases and asserts that:

- the open succeeds;
- every pre-existing row survives with the same content;
- new columns arrive at their documented default and backfills ran;
- a second open changes nothing.

Unit tests for a single backfill live next to `migrate()` in `storage.rs`.

## Config TOML

### Field-level rules

- New fields use `#[serde(default = ...)]` so configs without the key keep
  deserializing.
- Every setting is documented inline in the rendered default config.
- Renames and removals go through `config_migrate.rs`: add a deprecated-key
  migration that rewrites the old key to its replacement. The migration is the
  audit trail, even where serde would accept the old shape.
- Migrations operate on a `toml_edit::DocumentMut`, so user comments and
  formatting survive. They are applied in memory at every entrypoint and
  persisted by the TUI.

### User data wins

If a migration cannot interpret a value, it leaves it in place rather than
discarding it. A user-customized block is never pruned, even for a retired
provider.

### Required test

Config upgrades are covered in `crates/dux-core/tests/upgrade_config.rs` and the
unit tests in `config_migrate.rs`: load an old config and assert the migrated
shape and that no user value was lost.

## Review checklist

- [ ] New column appears in both `create table if not exists` and an
      `ensure_column` call.
- [ ] Column is nullable or defaulted, and the default is safe for old rows.
- [ ] Changes sit in the workstream's own appended block in `migrate()`.
- [ ] Backfills are idempotent (second open is a no-op).
- [ ] `upgrade_database.rs` or `upgrade_config.rs` covers the change.
- [ ] Renames or drops have a deprecation log line and a release-note entry.
