//! Integration tests for versioned SQLite/config migrations, including the
//! crash-atomic shared-workspace identity rebuild in migration 0005.
//!
//! These tests live under `tests/` so they exercise the public
//! [`dux::storage::SessionStore`] / [`dux::config::migrate_config`]
//! APIs the same way an external consumer (or a future doctor tool)
//! would. They assert behaviour, not implementation details: that
//! migrations bump `PRAGMA user_version`, that a second run is a
//! no-op, and that an old `Config` parses and is upgraded to the
//! current schema by [`migrate_config`].

use dux::config::{CONFIG_SCHEMA_CURRENT, Config, DuxPaths, ensure_config, migrate_config};
use dux::storage::SessionStore;
use rusqlite::{Connection, params};

fn create_v4_database(path: &std::path::Path, seed_sql: &str) {
    let conn = Connection::open(path).expect("open raw v4 database");
    conn.pragma_update(None, "foreign_keys", false)
        .expect("disable foreign keys while constructing legacy fixture");
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
    conn.execute_batch(include_str!(
        "../src/storage/migrations/0004_session_sort_order.sql"
    ))
    .expect("apply 0004");
    conn.execute_batch(seed_sql).expect("seed v4 database");
    conn.pragma_update(None, "user_version", 4)
        .expect("stamp v4");
}

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
        "provider_session_ids",
        "status",
        "state_json",
        "session_settings",
        "sort_order",
        "shared_workspace",
        "agent_handle",
        "deleted_at",
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

#[test]
fn migration_0006_adds_provider_session_ids_to_v5_database() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("v5.sqlite3");
    drop(SessionStore::open(&path).expect("create current database"));

    let conn = Connection::open(&path).expect("open raw database");
    conn.execute_batch(
        r#"
        alter table agent_sessions drop column provider_session_ids;
        pragma user_version = 5;
        insert into agent_sessions
            (id, project_id, provider, source_branch, branch_name,
             worktree_path, agent_handle, status, created_at, updated_at)
        values
            ('legacy', 'project', 'claude', 'main', 'legacy',
             '/tmp/legacy', 'legacy', 'detached',
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        "#,
    )
    .expect("construct v5 fixture");
    let old_columns: Vec<String> = conn
        .prepare("pragma table_info(agent_sessions)")
        .expect("prepare old table_info")
        .query_map([], |row| row.get(1))
        .expect("query old table_info")
        .collect::<rusqlite::Result<_>>()
        .expect("collect old columns");
    assert!(!old_columns.contains(&"provider_session_ids".to_string()));
    drop(conn);

    let store = SessionStore::open(&path).expect("migrate v5 to v6");
    assert_eq!(
        store.schema_version().expect("schema version"),
        SessionStore::CURRENT_SCHEMA_VERSION
    );
    let provider_session_ids: String = store
        .conn()
        .query_row(
            "select provider_session_ids from agent_sessions where id = 'legacy'",
            [],
            |row| row.get(0),
        )
        .expect("read migrated provider map");
    assert_eq!(provider_session_ids, "{}");
}

#[test]
fn migration_0005_backfills_duplicate_handles_deterministically() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("duplicates.sqlite3");
    create_v4_database(
        &path,
        r#"
        insert into agent_sessions
            (id, project_id, provider, source_branch, branch_name,
             worktree_path, status, created_at, updated_at)
        values
            ('b', 'p', 'claude', 'main', 'second', '/two/Same Name',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
            ('a', 'p', 'claude', 'main', 'first', '/one/Same Name',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
            ('c', 'p', 'claude', 'main', '???', '/three/---',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        "#,
    );

    let store = SessionStore::open(&path).expect("migrate v4 to current schema");
    let version: u32 = store
        .conn()
        .query_row("pragma user_version", [], |row| row.get(0))
        .expect("read current schema version");
    assert_eq!(version, SessionStore::CURRENT_SCHEMA_VERSION);
    let handles: Vec<(String, String)> = store
        .conn()
        .prepare("select id, agent_handle from agent_sessions order by id")
        .expect("prepare handles")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query handles")
        .collect::<rusqlite::Result<_>>()
        .expect("collect handles");
    assert_eq!(
        handles,
        vec![
            ("a".to_string(), "same-name".to_string()),
            ("b".to_string(), "same-name-2".to_string()),
            ("c".to_string(), "c".to_string()),
        ]
    );
}

#[test]
fn migration_0005_rebuild_enforces_handle_constraints_and_preserves_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("constraints.sqlite3");
    let store = SessionStore::open(&path).expect("open v5 store");
    let conn = store.conn();
    let insert = |id: &str, handle: Option<&str>| {
        conn.execute(
            r#"
            insert into agent_sessions
                (id, project_id, provider, source_branch, branch_name,
                 worktree_path, agent_handle, status, created_at, updated_at)
            values (?1, 'p', 'claude', 'main', ?1, '/tmp/wt', ?2,
                    'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')
            "#,
            params![id, handle],
        )
    };

    assert!(
        insert("null", None).is_err(),
        "agent_handle must be NOT NULL"
    );
    insert("valid", Some("valid")).expect("insert valid handle");
    assert!(
        insert("duplicate", Some("valid")).is_err(),
        "agent_handle must be UNIQUE"
    );
    for invalid in ["", "Bad/handle", &"x".repeat(65)] {
        assert!(
            insert(&format!("invalid-{}", invalid.len()), Some(invalid)).is_err(),
            "CHECK must reject {invalid:?}"
        );
    }
    let sort_index_count: i64 = conn
        .query_row(
            "select count(*) from sqlite_master where type = 'index' and name = 'idx_agent_sessions_sort_order'",
            [],
            |row| row.get(0),
        )
        .expect("query sort index");
    assert_eq!(sort_index_count, 1, "table rebuild must recreate indexes");
}

#[test]
fn migration_0005_preserves_session_pr_foreign_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("foreign-key.sqlite3");
    create_v4_database(
        &path,
        r#"
        insert into agent_sessions
            (id, project_id, provider, source_branch, branch_name,
             worktree_path, status, created_at, updated_at)
        values
            ('session', 'p', 'claude', 'main', 'branch', '/tmp/session',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        insert into session_prs (session_id, pr_number, owner_repo)
        values ('session', 42, 'owner/repo');
        "#,
    );

    let store = SessionStore::open(&path).expect("migrate with PR row");
    assert_eq!(store.load_prs("session").expect("load PRs").len(), 1);
    let fk: (String, String, String, String) = store
        .conn()
        .query_row("pragma foreign_key_list(session_prs)", [], |row| {
            Ok((row.get(2)?, row.get(3)?, row.get(4)?, row.get(6)?))
        })
        .expect("foreign key definition");
    assert_eq!(
        fk,
        (
            "agent_sessions".to_string(),
            "session_id".to_string(),
            "id".to_string(),
            "CASCADE".to_string(),
        )
    );
    let violations: i64 = store
        .conn()
        .query_row("select count(*) from pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .expect("foreign_key_check");
    assert_eq!(violations, 0);
    store.delete_session("session").expect("hard delete parent");
    assert!(
        store
            .load_prs("session")
            .expect("load cascaded PRs")
            .is_empty()
    );
}

#[test]
fn migration_0005_drops_orphan_session_prs_and_succeeds() {
    // A legacy DB can hold a session_prs row whose parent session is gone
    // (cascade didn't fire). The migration must DROP the orphan and succeed,
    // never abort — aborting would brick TUI launch (App::new opens with `?`).
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("orphan.sqlite3");
    create_v4_database(
        &path,
        r#"
        insert into agent_sessions
            (id, project_id, provider, source_branch, branch_name,
             worktree_path, status, created_at, updated_at)
        values
            ('live', 'p', 'claude', 'main', 'branch', '/tmp/live',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        insert into session_prs (session_id, pr_number, owner_repo)
        values ('live', 1, 'owner/repo'), ('missing-parent', 7, 'owner/repo');
        "#,
    );

    let store = SessionStore::open(&path).expect("migration drops the orphan and succeeds");
    // The live session's PR survives; the orphan is gone.
    assert_eq!(store.load_prs("live").expect("live PRs").len(), 1);
    let total_prs: i64 = store
        .conn()
        .query_row("select count(*) from session_prs", [], |row| row.get(0))
        .expect("count PRs");
    assert_eq!(total_prs, 1, "orphan session_prs row must be dropped");
    let version: u32 = store
        .conn()
        .query_row("pragma user_version", [], |row| row.get(0))
        .expect("version");
    assert_eq!(version, SessionStore::CURRENT_SCHEMA_VERSION);
}

#[test]
fn migration_0005_rolls_back_atomically_on_failure() {
    // Force a genuine mid-migration failure (a leftover agent_sessions_new
    // table makes `create table agent_sessions_new` fail) and prove the whole
    // rebuild + version bump rolls back — user_version stays 4, no new column,
    // no temp table. This would fail if the migration were not one transaction.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rollback.sqlite3");
    create_v4_database(
        &path,
        r#"
        create table agent_sessions_new (leftover text);
        "#,
    );

    let err = match SessionStore::open(&path) {
        Ok(_) => panic!("migration must fail"),
        Err(err) => err,
    };
    assert!(format!("{err:#}").contains("migration 5 failed"));

    let conn = Connection::open(&path).expect("reopen rolled-back database");
    let version: u32 = conn
        .query_row("pragma user_version", [], |row| row.get(0))
        .expect("read rolled-back version");
    assert_eq!(version, 4);
    let columns: Vec<String> = conn
        .prepare("pragma table_info(agent_sessions)")
        .expect("prepare old columns")
        .query_map([], |row| row.get(1))
        .expect("query old columns")
        .collect::<rusqlite::Result<_>>()
        .expect("collect old columns");
    assert!(!columns.contains(&"agent_handle".to_string()));
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

#[test]
fn config_v1_removes_only_the_legacy_open_worktree_binding() {
    let mut old = Config {
        schema_version: 1,
        ..Config::default()
    };
    old.keys
        .bindings
        .insert("open_worktree_in_editor".into(), vec!["o".into()]);

    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let paths = DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        root,
    };
    std::fs::write(
        &paths.config_path,
        toml::to_string(&old).expect("serialize v1 config"),
    )
    .expect("write v1 config");

    let migrated = ensure_config(&paths).expect("load and persist migration");
    assert_eq!(migrated.schema_version, CONFIG_SCHEMA_CURRENT);
    assert_eq!(
        migrated.keys.bindings["open_worktree_in_editor"],
        Vec::<String>::new()
    );
    let persisted: Config =
        toml::from_str(&std::fs::read_to_string(&paths.config_path).expect("read migrated config"))
            .expect("parse migrated config");
    assert_eq!(persisted.schema_version, CONFIG_SCHEMA_CURRENT);
    assert!(persisted.keys.bindings["open_worktree_in_editor"].is_empty());

    let mut customized = Config {
        schema_version: 1,
        ..Config::default()
    };
    customized
        .keys
        .bindings
        .insert("open_worktree_in_editor".into(), vec!["ctrl-o".into()]);

    let migrated = migrate_config(customized);
    assert_eq!(
        migrated.keys.bindings["open_worktree_in_editor"],
        vec!["ctrl-o"]
    );
}

#[test]
fn config_v2_moves_default_codex_scrollback_into_dux() {
    let mut old = Config {
        schema_version: 2,
        ..Config::default()
    };
    let codex = old
        .providers
        .commands
        .get_mut("codex")
        .expect("codex provider");
    codex.command = "codex-amq".into();
    codex.args.clear();
    codex.resume_args = Some(vec!["resume".into(), "--last".into()]);
    codex.resume_by_id_args = Some(vec!["resume".into(), "{session_id}".into()]);
    codex.forward_scroll = true;

    let mut customized = old.clone();
    customized.providers.commands["codex"].args = vec!["--model".into(), "o3".into()];

    let migrated = migrate_config(old);
    let codex = &migrated.providers.commands["codex"];
    assert_eq!(codex.args, ["--no-alt-screen"]);
    assert_eq!(
        codex.resume_args.as_deref().expect("resume args"),
        ["--no-alt-screen", "resume", "--last"]
    );
    assert_eq!(
        codex
            .resume_by_id_args
            .as_deref()
            .expect("resume-by-id args"),
        ["--no-alt-screen", "resume", "{session_id}"]
    );
    assert!(!codex.forward_scroll);

    let customized = migrate_config(customized);
    assert_eq!(
        customized.providers.commands["codex"].args,
        ["--model", "o3"]
    );
}
