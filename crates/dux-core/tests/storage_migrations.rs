//! Fork `tests/storage_migrations.rs`, ported under the fork's test names.
//!
//! The fork versioned its schema with numbered migrations and
//! `PRAGMA user_version`; upstream's storage has none (every `migrate()` step
//! is an idempotent `ensure_column` / `create ... if not exists`, run on every
//! open). So each fork test here asserts the INVARIANT its migration existed
//! for, against the shape a real user's database has: databases are built by
//! running the fork's own migration SQL (`fixtures/fork_main_schema/`, copied
//! verbatim from `git show main:src/storage/migrations/*`), stopped at the
//! schema version the fork test started from.

use dux_core::storage::SessionStore;
use rusqlite::{Connection, params};

const FORK_MIGRATIONS: &[&str] = &[
    include_str!("fixtures/fork_main_schema/0001_initial_schema.sql"),
    include_str!("fixtures/fork_main_schema/0002_session_state_v2.sql"),
    include_str!("fixtures/fork_main_schema/0003_session_settings.sql"),
    include_str!("fixtures/fork_main_schema/0004_session_sort_order.sql"),
];

/// A database exactly as fork main left it at schema `version` (1..=4), with
/// `seed_sql` run after the migrations. Foreign keys are off, as they were on
/// any database a pre-FK fork build wrote, so an orphan row can be seeded.
fn fork_database_at(path: &std::path::Path, version: usize, seed_sql: &str) {
    let conn = Connection::open(path).expect("open raw fork database");
    conn.pragma_update(None, "foreign_keys", false)
        .expect("foreign keys off for the legacy fixture");
    for sql in &FORK_MIGRATIONS[..version] {
        conn.execute_batch(sql).expect("apply fork migration");
    }
    conn.execute_batch(seed_sql).expect("seed fork database");
    conn.pragma_update(None, "user_version", version as i64)
        .expect("stamp fork schema version");
}

fn columns(path: &std::path::Path, table: &str) -> Vec<String> {
    Connection::open(path)
        .expect("raw open")
        .prepare(&format!("pragma table_info({table})"))
        .expect("prepare table_info")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query table_info")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect columns")
}

/// Every column the upsert and load paths read or write, which the migration
/// must leave in place (a missing one panics at runtime, not in a test).
const REQUIRED_SESSION_COLUMNS: &[&str] = &[
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
    "session_settings",
    "sort_order",
    "shared_workspace",
    "agent_handle",
    "deleted_at",
    "provider_session_ids",
    "created_at",
    "updated_at",
];

/// Fork: a fresh database gets the whole schema. Upstream has no
/// `user_version`; the observable contract is the same: after `open` every
/// column the storage layer uses exists, and the tables it writes are usable.
#[test]
fn migrate_from_empty_db_runs_all_migrations() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("fresh.sqlite3");
    let store = SessionStore::open(&path).expect("open fresh DB");

    let have = columns(&path, "agent_sessions");
    for required in REQUIRED_SESSION_COLUMNS {
        assert!(
            have.iter().any(|c| c == required),
            "agent_sessions missing column {required} after migration; columns = {have:?}"
        );
    }
    for table in ["session_prs", "agent_tabs", "projects", "app_state"] {
        assert!(
            !columns(&path, table).is_empty(),
            "table {table} was not created"
        );
    }
    assert!(store.load_sessions().expect("load").is_empty());
}

/// Fork: re-running the migration on a migrated database is a no-op. Without
/// `user_version` the stable fact is the schema itself (every table, column
/// and index) plus the rows, compared across a second open.
#[test]
fn migrate_idempotent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("idem.sqlite3");
    let schema = |path: &std::path::Path| -> Vec<(String, Option<String>)> {
        Connection::open(path)
            .expect("raw open")
            .prepare("select name, sql from sqlite_master order by type, name")
            .expect("prepare schema")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query schema")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect schema")
    };

    drop(SessionStore::open(&path).expect("first open"));
    let first = schema(&path);
    drop(SessionStore::open(&path).expect("second open"));
    assert_eq!(
        first,
        schema(&path),
        "a second open must not add, drop or change any table, column or index"
    );
}

/// Fork: a database from before `sort_order` existed (fork schema 3) keeps
/// the order the user last saw, most recently updated first, when the column
/// is introduced.
#[test]
fn migrate_v3_to_v4_backfills_session_sort_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v3.sqlite3");
    fork_database_at(
        &path,
        3,
        r#"
        insert into agent_sessions
            (id, project_id, provider, source_branch, branch_name, worktree_path, status, created_at, updated_at)
        values
            ('old', 'p', 'claude', 'main', 'old-branch', '/tmp/old', 'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
            ('new', 'p', 'claude', 'main', 'new-branch', '/tmp/new', 'detached', '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z');
        "#,
    );

    let store = SessionStore::open(&path).expect("open a fork schema-3 database");
    let ids: Vec<String> = store
        .load_sessions()
        .expect("load sessions")
        .into_iter()
        .map(|session| session.id)
        .collect();
    assert_eq!(ids, vec!["new".to_string(), "old".to_string()]);

    // The ORDER alone cannot tell a backfill from the loader's own tiebreak
    // (`updated_at desc`), so the persisted positions are the subject.
    let orders: Vec<(String, i64)> = Connection::open(&path)
        .expect("raw open")
        .prepare("select id, sort_order from agent_sessions order by id")
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("collect");
    assert_eq!(
        orders,
        vec![("new".to_string(), 0), ("old".to_string(), 1)],
        "the backfill writes a distinct position per session"
    );
}

/// Fork migration 0005: legacy rows get handles derived from the directory
/// basename, deterministically (visited in id order), suffixed on a
/// collision, falling back to the id when nothing else normalizes.
#[test]
fn migration_0005_backfills_duplicate_handles_deterministically() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("duplicates.sqlite3");
    fork_database_at(
        &path,
        4,
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

    let store = SessionStore::open(&path).expect("open a fork schema-4 database");
    let handles: Vec<(String, String)> = Connection::open(&path)
        .expect("raw open")
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
    assert_eq!(store.load_sessions().expect("load").len(), 3);
}

/// Fork migration 0005 dropped a `session_prs` row whose session was gone
/// rather than aborting (an abort bricked startup). Upstream keeps foreign
/// keys off, so such a row can no longer abort anything; the invariant is
/// that the database opens and the orphan cannot surface as anyone's pull
/// request, while the live session's PR survives.
#[test]
fn migration_0005_drops_orphan_session_prs_and_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("orphan.sqlite3");
    fork_database_at(
        &path,
        4,
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

    let store = SessionStore::open(&path).expect("an orphan PR row must not fail the open");
    assert_eq!(store.load_prs("live").expect("live PRs").len(), 1);
    let session_ids: Vec<String> = store
        .load_sessions()
        .expect("load")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(session_ids, vec!["live".to_string()]);
    let visible: Vec<String> = store
        .load_all_latest_prs()
        .expect("latest PRs")
        .into_iter()
        .map(|pr| pr.session_id)
        .filter(|id| session_ids.contains(id))
        .collect();
    assert_eq!(visible, vec!["live".to_string()]);
    let orphans: i64 = Connection::open(&path)
        .expect("raw open")
        .query_row(
            "select count(*) from session_prs where session_id not in (select id from agent_sessions)",
            [],
            |row| row.get(0),
        )
        .expect("count orphans");
    assert_eq!(orphans, 0, "the orphan session_prs row must be dropped");
}

/// Fork migration 0005 rebuilt `agent_sessions` and had to keep
/// `session_prs`' reference to it (a rebuild that re-points or loses the FK
/// silently breaks the cascade). Upstream does not rebuild tables and keeps
/// foreign keys off, deleting child rows explicitly; the invariant is that
/// the PR row survives the upgrade and a hard delete of its session takes it
/// with it.
#[test]
fn migration_0005_preserves_session_pr_foreign_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("foreign-key.sqlite3");
    fork_database_at(
        &path,
        4,
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
    let fk: (String, String, String) = Connection::open(&path)
        .expect("raw open")
        .query_row("pragma foreign_key_list(session_prs)", [], |row| {
            Ok((row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .expect("foreign key definition kept");
    assert_eq!(
        fk,
        (
            "agent_sessions".to_string(),
            "session_id".to_string(),
            "id".to_string()
        ),
        "the declared reference still names agent_sessions(id)"
    );
    store.delete_session("session").expect("hard delete parent");
    assert!(store.load_prs("session").expect("load").is_empty());
}

/// Fork migration 0005 rebuilt the table with a NOT NULL, UNIQUE, CHECKed
/// `agent_handle` and had to recreate the sort index. SQLite cannot add a
/// CHECK to an existing table, so upstream enforces the same contract in
/// Rust plus a partial unique index. Asserted through the public API (which
/// is how every row is written) and through raw SQL for what the database
/// itself refuses.
#[test]
fn migration_0005_rebuild_enforces_handle_constraints_and_preserves_index() {
    use chrono::Utc;
    use dux_core::model::{
        AgentSession, AgentWorkspace, BranchProvenance, ManagedWorkspace, ProviderKind,
        SessionStatus,
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("constraints.sqlite3");
    fork_database_at(&path, 4, "");
    let store = SessionStore::open(&path).expect("open a fork schema-4 database");
    let session = |id: &str, handle: &str| {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            agent_handle: handle.to_string(),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: format!("{id}-slot"),
            provider: ProviderKind::new("claude"),
            workspace: AgentWorkspace::Managed(ManagedWorkspace {
                project_id: "p".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: id.to_string(),
                initial_branch: id.to_string(),
                branch_provenance: BranchProvenance::CreatedByDux,
                worktree_path: format!("/tmp/{id}"),
            }),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: false,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    };

    store
        .create_session(&session("valid", "valid"))
        .expect("insert a valid handle");
    assert!(
        store.create_session(&session("dup", "valid")).is_err(),
        "agent_handle must be unique"
    );
    for invalid in ["", "Bad/handle", &"x".repeat(65)] {
        assert!(
            store
                .create_session(&session(&format!("bad-{}", invalid.len()), invalid))
                .is_err(),
            "the handle contract must reject {invalid:?}"
        );
    }
    let conn = Connection::open(&path).expect("raw open");
    assert!(
        conn.execute(
            "insert into agent_sessions (id, project_id, provider, source_branch, branch_name, \
             worktree_path, status, created_at, updated_at, agent_handle) \
             values ('raw', 'p', 'claude', 'main', 'raw', '/tmp/raw', 'detached', \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', ?1)",
            params!["valid"],
        )
        .is_err(),
        "the database itself refuses a duplicate handle"
    );
    let index_count: i64 = conn
        .query_row(
            "select count(*) from sqlite_master where type = 'index' \
             and name = 'idx_agent_sessions_sort_order'",
            [],
            |row| row.get(0),
        )
        .expect("query sort index");
    assert_eq!(index_count, 1, "the fork's sort index survives the upgrade");
    assert_eq!(store.load_sessions().expect("load").len(), 1);
}

/// Fork migration 0005 ran as one transaction so a failure left the database
/// exactly as it was. Upstream's only multi-row step is the handle backfill,
/// which runs in ONE transaction: a failure part-way leaves no row with a
/// handle, and the next open retries from the same state and succeeds.
#[test]
fn migration_0005_rolls_back_atomically_on_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("rollback.sqlite3");
    fork_database_at(
        &path,
        4,
        r#"
        insert into agent_sessions
            (id, project_id, provider, source_branch, branch_name,
             worktree_path, status, created_at, updated_at)
        values
            ('a', 'p', 'claude', 'main', 'a', '/tmp/first',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
            ('b', 'p', 'claude', 'main', 'b', '/tmp/second',
             'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        "#,
    );
    // Make the SECOND assignment fail after the first has been written: a
    // trigger aborts the update that would give row 'b' its handle.
    {
        let conn = Connection::open(&path).expect("raw open");
        conn.execute_batch(
            "alter table agent_sessions add column agent_handle text not null default '';
             create trigger fail_second_handle before update of agent_handle on agent_sessions
             when new.id = 'b' begin select raise(abort, 'injected backfill failure'); end;",
        )
        .expect("install failure");
    }

    let err = match SessionStore::open(&path) {
        Ok(_) => panic!("the injected failure must fail the open"),
        Err(err) => err,
    };
    assert!(
        format!("{err:#}").contains("injected backfill failure"),
        "{err:#}"
    );
    let assigned: i64 = Connection::open(&path)
        .expect("raw open")
        .query_row(
            "select count(*) from agent_sessions where agent_handle <> ''",
            [],
            |row| row.get(0),
        )
        .expect("count assigned");
    assert_eq!(
        assigned, 0,
        "the whole backfill rolled back, row 'a' included"
    );

    Connection::open(&path)
        .expect("raw open")
        .execute_batch("drop trigger fail_second_handle;")
        .expect("remove failure");
    let store = SessionStore::open(&path).expect("the retry succeeds");
    let handles: Vec<String> = store
        .load_sessions()
        .expect("load")
        .into_iter()
        .map(|s| s.agent_handle().to_string())
        .collect();
    assert_eq!(handles.len(), 2);
    assert!(handles.iter().all(|h| !h.is_empty()));
}
