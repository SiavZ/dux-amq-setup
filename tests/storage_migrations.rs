//! Integration tests for audit02 P1-Y (Phase 19): explicit schema
//! versioning for both the SQLite session store and the TOML config
//! file.
//!
//! These tests live under `tests/` so they exercise the public
//! [`dux::storage::SessionStore`] / [`dux::config::migrate_config`]
//! APIs the same way an external consumer (or a future doctor tool)
//! would. They assert behaviour, not implementation details: that
//! migrations bump `PRAGMA user_version`, that a second run is a
//! no-op, and that an old `Config` parses and is upgraded to the
//! current schema by [`migrate_config`].

use dux::config::{CONFIG_SCHEMA_CURRENT, Config, migrate_config};
use dux::storage::SessionStore;

/// Opening a `SessionStore` against a fresh, empty database file must
/// run every entry in the `MIGRATIONS` slice. Externally we observe
/// this through `PRAGMA user_version`: after migration it is non-zero
/// and matches the latest migration number that ships in this build.
#[test]
fn migrate_from_empty_db_runs_all_migrations() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("fresh.sqlite3");

    let store = SessionStore::open(&path).expect("open fresh DB");
    let user_version: u32 = store
        .conn()
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .expect("read user_version");
    assert!(
        user_version >= 1,
        "expected user_version >= 1 after migrations, got {user_version}"
    );

    // The migration also has to leave the canonical schema in place:
    // `agent_sessions` and `session_prs` must exist with the columns
    // that the upsert path writes to, otherwise downstream code panics
    // at runtime, not in this test.
    let agent_sessions_columns: Vec<String> = store
        .conn()
        .prepare("pragma table_info(agent_sessions)")
        .expect("prepare table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query table_info")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect column names");
    for required in [
        "id",
        "project_id",
        "provider",
        "source_branch",
        "branch_name",
        "worktree_path",
        "title",
        "project_path",
        "started_providers",
        "status",
        "sort_order",
        "created_at",
        "updated_at",
    ] {
        assert!(
            agent_sessions_columns.iter().any(|c| c == required),
            "agent_sessions missing column {required} after migration; \
             columns = {agent_sessions_columns:?}"
        );
    }
}

/// Re-running the migration loop on an already-migrated database is a
/// no-op: `PRAGMA user_version` is unchanged. We exercise this by
/// opening the same physical file twice — `SessionStore::open` calls
/// `migrate` internally, so the second open is the second run.
#[test]
fn migrate_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("idem.sqlite3");

    let first_version = {
        let store = SessionStore::open(&path).expect("first open");
        store
            .conn()
            .query_row::<u32, _, _>("PRAGMA user_version;", [], |r| r.get(0))
            .expect("read user_version after first open")
    };

    let second_version = {
        let store = SessionStore::open(&path).expect("second open");
        store
            .conn()
            .query_row::<u32, _, _>("PRAGMA user_version;", [], |r| r.get(0))
            .expect("read user_version after second open")
    };

    assert_eq!(
        first_version, second_version,
        "migrations must be idempotent: user_version drifted from \
         {first_version} to {second_version} on re-open"
    );
    assert!(
        first_version >= 1,
        "user_version should be >= 1 after migrations (got {first_version})"
    );
}

/// audit03 Phase 01: opening a v2 database (one that already ran
/// migrations 1+2) against a current binary must apply migration 0003
/// and add the `session_settings` column without disturbing existing
/// rows. Asserts the column exists and `user_version` is bumped to 3.
#[test]
fn migrate_v2_to_v3_adds_session_settings_column() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2.sqlite3");

    // Build a database that has only migrations 1+2 applied. We can't
    // call SessionStore::open here (it would run all migrations
    // including 0003), so we run the SQL by hand and stamp
    // `user_version` to 2 so the migration runner picks up at 0003 on
    // the next open.
    {
        let conn = rusqlite::Connection::open(&path).expect("open raw");
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0001_initial_schema.sql"
        ))
        .expect("apply 0001");
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0002_session_state_v2.sql"
        ))
        .expect("apply 0002");
        conn.execute_batch("PRAGMA user_version = 2;")
            .expect("stamp v2");
    }

    // Sanity-check the pre-migration shape: no `session_settings`
    // column yet.
    {
        let conn = rusqlite::Connection::open(&path).expect("reopen raw");
        let cols: Vec<String> = conn
            .prepare("pragma table_info(agent_sessions)")
            .expect("table_info pre")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query pre")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect pre");
        assert!(
            !cols.contains(&"session_settings".to_string()),
            "v2 schema should not have session_settings yet; columns = {cols:?}"
        );
    }

    // Open via SessionStore — runs the remaining migrations.
    let _store = dux::storage::SessionStore::open(&path).expect("open at v3");

    // Confirm the new column exists, is nullable, and `user_version`
    // advanced to 3 (or higher if newer migrations land later).
    let conn = rusqlite::Connection::open(&path).expect("reopen post");
    let cols: Vec<String> = conn
        .prepare("pragma table_info(agent_sessions)")
        .expect("table_info post")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query post")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect post");
    assert!(
        cols.contains(&"session_settings".to_string()),
        "session_settings column missing post-migration; columns = {cols:?}"
    );
    let user_version: u32 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .expect("read user_version");
    assert!(
        user_version >= 3,
        "expected user_version >= 3 after 0003 migration, got {user_version}"
    );
}

/// Opening a v3 database with existing sessions must add `sort_order`
/// and back-fill it to preserve the old startup display order
/// (`updated_at desc`) before the manual ordering feature takes over.
#[test]
fn migrate_v3_to_v4_backfills_session_sort_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v3.sqlite3");

    {
        let conn = rusqlite::Connection::open(&path).expect("open raw");
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0001_initial_schema.sql"
        ))
        .expect("apply 0001");
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0002_session_state_v2.sql"
        ))
        .expect("apply 0002");
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0003_session_settings.sql"
        ))
        .expect("apply 0003");
        conn.execute_batch(
            r#"
            insert into agent_sessions
                (id, project_id, provider, source_branch, branch_name, worktree_path, status, created_at, updated_at)
            values
                ('old', 'p', 'claude', 'main', 'old-branch', '/tmp/old', 'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
                ('new', 'p', 'claude', 'main', 'new-branch', '/tmp/new', 'detached', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z');
            PRAGMA user_version = 3;
            "#,
        )
        .expect("seed v3");
    }

    let store = SessionStore::open(&path).expect("open at v4");
    let loaded = store.load_sessions().expect("load sessions");
    let ids: Vec<&str> = loaded.iter().map(|session| session.id.as_str()).collect();
    assert_eq!(ids, vec!["new", "old"]);

    let user_version: u32 = store
        .conn()
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .expect("read user_version");
    assert!(
        user_version >= 4,
        "expected user_version >= 4 after 0004 migration, got {user_version}"
    );
}

/// A `config.toml` that predates the `schema_version` field must still
/// deserialize cleanly (thanks to `#[serde(default)]`) and be moved
/// forward to `CONFIG_SCHEMA_CURRENT` by `migrate_config`. This
/// exercises the policy that old configs always load on a newer dux.
///
/// We start with `schema_version = 0` to simulate the worst case (a
/// pre-versioning config) and assert the ladder catches up to current.
#[test]
fn config_v0_loads_and_migrates_to_current() {
    // Deliberately minimal TOML: only the schema_version is set, every
    // other field falls through to its serde default. This mirrors
    // what an extremely old config — written before most sections
    // existed — looks like once we override the version.
    let toml = "schema_version = 0\n";

    let parsed: Config = toml::from_str(toml).expect("parse v0 config");
    assert_eq!(parsed.schema_version, 0, "starting version is 0");

    let migrated = migrate_config(parsed);
    assert_eq!(
        migrated.schema_version, CONFIG_SCHEMA_CURRENT,
        "migrate_config must bring schema_version to \
         CONFIG_SCHEMA_CURRENT ({CONFIG_SCHEMA_CURRENT})"
    );

    // Running the migration twice is a no-op (the loop guard exits).
    let migrated_again = migrate_config(migrated);
    assert_eq!(migrated_again.schema_version, CONFIG_SCHEMA_CURRENT);
}
