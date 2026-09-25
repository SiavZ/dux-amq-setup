# Audits

Point-in-time audit reports of the fork (audit01, audit02, audit03 with its
disposition in `audit03/99-disposition.md`).

> **Historical record, pre-workspace paths.** File paths in these reports
> refer to the fork-main single-crate layout, before the `crates/`
> restructure. Mapping: `src/` -> `crates/dux-core/src/` (engine, config,
> storage, pty, git, model) or `crates/dux-tui/src/` (`src/app/`, `src/cli.rs`,
> keys, rendering); `tests/` -> `crates/*/tests/`. Storage no longer uses
> numbered migrations (`src/storage/migrations/000N_*.sql`, `PRAGMA
> user_version`): it uses idempotent `ensure_column` calls in
> `crates/dux-core/src/storage.rs`. See
> [docs/contributing/schema-policy.md](../contributing/schema-policy.md).
> The body is left as written.
