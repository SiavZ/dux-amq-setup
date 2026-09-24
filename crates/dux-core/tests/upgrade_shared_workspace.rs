//! Upgrade safety for the shared-workspace identity columns (`agent_handle`,
//! `shared_workspace`, `deleted_at`) ported from the fork.
//!
//! Two real starting points exist in the wild and both must open cleanly:
//!
//! 1. A database written by UPSTREAM dux: no identity columns at all. Every
//!    row must come out with a derived, valid, locally unique handle.
//! 2. A database written by FORK main, at fork schema version 6. The fork used
//!    a numbered migration runner (`PRAGMA user_version`) and rebuilt
//!    `agent_sessions` in its migration 0005. That database is built here by
//!    running the fork's ACTUAL migration files, copied verbatim into
//!    `tests/fixtures/fork_main_schema/` from `git show main:src/storage/migrations/*`,
//!    rather than a hand-written approximation, so the test exercises the
//!    shape real fork users have on disk. Its handles are referenced by AMQ
//!    inboxes on disk and must survive byte for byte.
//!
//! Migration 0005 in the fork splits at a marker so Rust can backfill handles
//! mid-migration. The fixture builder below reproduces exactly that runner
//! step (insert every legacy row with a derived handle between the halves),
//! because a fork user's database went through it.

use dux_core::model::is_valid_agent_handle;
use dux_core::storage::SessionStore;
use rusqlite::Connection;

const FORK_MIGRATIONS: &[&str] = &[
    include_str!("fixtures/fork_main_schema/0001_initial_schema.sql"),
    include_str!("fixtures/fork_main_schema/0002_session_state_v2.sql"),
    include_str!("fixtures/fork_main_schema/0003_session_settings.sql"),
    include_str!("fixtures/fork_main_schema/0004_session_sort_order.sql"),
    include_str!("fixtures/fork_main_schema/0005_shared_workspace.sql"),
    include_str!("fixtures/fork_main_schema/0006_provider_session_ids.sql"),
];
const FORK_HANDLE_BACKFILL_MARKER: &str = "-- rust-backfill-agent-handles";

/// Build a fork-main database at schema 6 the way the fork's runner did,
/// seeding legacy rows at schema 4 (before identity existed) so 0005's
/// backfill runs over real data, then adding post-0006 rows the way a fork
/// build writes them (tombstone, shared agent, handle that differs from the
/// worktree basename).
fn fork_main_database() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sessions.sqlite3");
    let mut conn = Connection::open(&path).expect("open");
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    for (index, sql) in FORK_MIGRATIONS.iter().enumerate() {
        let version = index + 1;
        if version == 5 {
            // Legacy rows exist before 0005 runs, as for any real fork user.
            // The orphan PR row is what a pre-FK database could hold; the
            // fork's 0005 drops it, and it can only be written with FKs off.
            conn.execute_batch(
                r#"
                PRAGMA foreign_keys = OFF;
                insert into agent_sessions
                  (id, project_id, provider, source_branch, branch_name, worktree_path,
                   title, project_path, started_providers, status, created_at, updated_at,
                   state_json, session_settings, sort_order)
                values
                  ('legacy-a', 'proj-1', 'claude', 'main', 'dux/lively-otter',
                   '/home/ada/.dux/worktrees/widget/Lively-Otter', 'Lively', '/home/ada/widget',
                   '["claude"]', 'detached', '2026-01-01T00:00:00Z', '2026-01-02T00:00:00Z',
                   null, null, 0),
                  ('legacy-b', 'proj-1', 'codex', 'main', 'dux/other',
                   '/home/ada/.dux/worktrees/gadget/lively-otter', null, '/home/ada/gadget',
                   '[]', 'exited', '2026-01-01T00:00:00Z', '2026-01-03T00:00:00Z',
                   null, null, 1);
                insert into session_prs (session_id, pr_number, owner_repo, state, title)
                values ('legacy-a', 7, 'ada/widget', 'OPEN', 'Add otter'),
                       ('ghost', 9, 'ada/widget', 'OPEN', 'orphan');
                PRAGMA foreign_keys = ON;
                "#,
            )
            .unwrap();
        }
        let tx = conn.transaction().unwrap();
        if version == 5 {
            let (before, after) = sql.split_once(FORK_HANDLE_BACKFILL_MARKER).unwrap();
            tx.execute_batch(before).unwrap();
            // The fork runner's backfill: basename-first handle, `-N` on a
            // collision, rows visited in id order.
            tx.execute_batch(
                r#"
                insert into agent_sessions_new
                  (id, project_id, provider, source_branch, branch_name, worktree_path,
                   title, project_path, started_providers, status, created_at, updated_at,
                   state_json, session_settings, sort_order, shared_workspace, agent_handle,
                   deleted_at)
                select id, project_id, provider, source_branch, branch_name, worktree_path,
                       title, project_path, started_providers, status, created_at, updated_at,
                       state_json, session_settings, sort_order, 0,
                       case id when 'legacy-a' then 'lively-otter' else 'lively-otter-2' end,
                       null
                from agent_sessions;
                "#,
            )
            .unwrap();
            tx.execute_batch(after).unwrap();
        } else {
            tx.execute_batch(sql).unwrap();
        }
        tx.pragma_update(None, "user_version", version as i64)
            .unwrap();
        tx.commit().unwrap();
    }
    // Rows a fork build wrote after reaching schema 6.
    conn.execute_batch(
        r#"
        insert into agent_sessions
          (id, project_id, provider, source_branch, branch_name, worktree_path, title,
           project_path, started_providers, status, created_at, updated_at, state_json,
           session_settings, sort_order, shared_workspace, agent_handle, deleted_at,
           provider_session_ids)
        values
          ('shared-1', 'proj-1', 'claude', 'main', 'main', '/home/ada/widget', 'Shared',
           '/home/ada/widget', '[]', 'detached', '2026-02-01T00:00:00Z',
           '2026-02-02T00:00:00Z', null, null, 2, 1, 'widget', null,
           '{"claude":"0b7c1c4e-0000-4000-8000-000000000001"}'),
          ('gone-1', 'proj-1', 'claude', 'main', 'dux/gone', '/home/ada/.dux/worktrees/widget/gone',
           null, '/home/ada/widget', '[]', 'exited', '2026-02-01T00:00:00Z',
           '2026-02-03T00:00:00Z', null, null, 3, 0, 'gone', '2026-02-04T00:00:00+00:00', '{}');
        "#,
    )
    .unwrap();
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 6, "the fixture must be at fork schema 6");
    drop(conn);
    (tmp, path)
}

fn handles(path: &std::path::Path) -> Vec<(String, String, i64, Option<String>)> {
    let conn = Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare(
            "select id, agent_handle, shared_workspace, deleted_at from agent_sessions order by id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

#[test]
fn a_fork_main_schema_6_database_opens_and_keeps_every_identity_byte_for_byte() {
    let (_tmp, path) = fork_main_database();
    let before = handles(&path);

    let store = SessionStore::open(&path).expect("a fork-main database must open");
    assert_eq!(
        handles(&path),
        before,
        "handles, shared flags and tombstones are data AMQ inboxes depend on; \
         the upgrade must not touch them"
    );

    let live = store.load_sessions().expect("load live sessions");
    let live_ids: Vec<&str> = live.iter().map(|s| s.id.as_str()).collect();
    assert!(
        !live_ids.contains(&"gone-1"),
        "a fork tombstone must stay hidden after the upgrade: {live_ids:?}"
    );
    for id in ["legacy-a", "legacy-b", "shared-1"] {
        assert!(live_ids.contains(&id), "{id} lost in the upgrade");
    }
    let shared = live.iter().find(|s| s.id == "shared-1").unwrap();
    assert!(shared.shared_workspace());
    assert_eq!(shared.agent_handle(), "widget");
    let legacy_b = live.iter().find(|s| s.id == "legacy-b").unwrap();
    assert_eq!(legacy_b.agent_handle(), "lively-otter-2");
    assert_eq!(legacy_b.branch_name(), Some("dux/other"));

    let all = store
        .load_sessions_including_deleted()
        .expect("load with tombstones");
    let gone = all
        .iter()
        .find(|s| s.id == "gone-1")
        .expect("tombstone kept");
    assert!(gone.is_deleted());
    assert_eq!(gone.agent_handle(), "gone");

    // Upstream-only state the fork never had is created and usable.
    let prs = store.load_prs("legacy-a").expect("PR history survives");
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].title, "Add otter");
    assert_eq!(prs[0].host, "github.com");
    store.set_app_state("k", "v").expect("app_state created");
    assert_eq!(
        store.count_agent_tabs("legacy-a").unwrap(),
        1,
        "slot tab minted"
    );
    assert!(
        store
            .load_projects()
            .expect("projects table created")
            .is_empty(),
        "the fork kept projects in config.toml, so the table starts empty"
    );
}

#[test]
fn a_fork_main_database_survives_a_second_open_and_a_fresh_create_cannot_steal_a_handle() {
    let (_tmp, path) = fork_main_database();
    drop(SessionStore::open(&path).unwrap());
    let after_first = handles(&path);
    let store = SessionStore::open(&path).expect("second open is a no-op");
    assert_eq!(handles(&path), after_first);

    // A new agent whose worktree is also named `gone` must not take the
    // tombstone's handle: the tombstone's inbox may still be on disk.
    let mut session = store.load_sessions().unwrap().remove(0);
    session.id = "new-1".to_string();
    session.slot_tab_id = "new-1-slot".to_string();
    session.agent_handle = "gone".to_string();
    store.assign_unique_agent_handle(&mut session).unwrap();
    assert_eq!(session.agent_handle(), "gone-2");
    store.create_session(&session).unwrap();
}

#[test]
fn an_upstream_database_gets_derived_valid_unique_handles_for_every_row() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sessions.sqlite3");
    // A current upstream database: created by this crate's own migrate()
    // before identity existed is not reproducible here, so strip the identity
    // columns from a freshly created one instead. That is exactly upstream's
    // shape: every other column and table is what upstream writes.
    {
        let store = SessionStore::open(&path).unwrap();
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            drop index idx_agent_sessions_agent_handle;
            alter table agent_sessions drop column agent_handle;
            alter table agent_sessions drop column shared_workspace;
            alter table agent_sessions drop column deleted_at;
            insert into agent_sessions
              (id, project_id, provider, source_branch, branch_name, worktree_path, status,
               created_at, updated_at, workspace_kind, folder_path, sort_order)
            values
              ('u1', 'p', 'claude', 'main', 'dux/Feature Login', '/wt/p/Feature Login',
               'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'managed', null, 0),
              ('u2', 'p', 'claude', 'main', 'dux/other', '/wt/q/feature-login',
               'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'managed', null, 1),
              ('u3', '', 'claude', '', '', '', 'active', '2026-01-01T00:00:00Z',
               '2026-01-01T00:00:00Z', 'folder', '/home/ada/Notes', 2),
              ('u4', 'p', 'claude', 'main', '!!!', '/wt/p/!!!',
               'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'managed', null, 3);
            "#,
        )
        .unwrap();
    }
    let store = SessionStore::open(&path).expect("an upstream database must open");
    let rows = handles(&path);
    let by_id = |id: &str| rows.iter().find(|r| r.0 == id).unwrap().1.clone();
    assert_eq!(by_id("u1"), "feature-login");
    assert_eq!(by_id("u2"), "feature-login-2", "collision suffixed");
    assert_eq!(
        by_id("u3"),
        "notes",
        "a folder agent derives from its folder"
    );
    assert_eq!(
        by_id("u4"),
        "u4",
        "falls through basename and branch to the id"
    );
    for (_, handle, shared, deleted) in &rows {
        assert!(is_valid_agent_handle(handle), "{handle:?}");
        assert_eq!(*shared, 0);
        assert!(deleted.is_none());
    }
    assert_eq!(store.load_sessions().unwrap().len(), 4);
}

/// Fork efb8224e/5ba99d54 (`tests/session_state.rs`): the fork persisted a
/// typed session state in `state_json`, including an interrupted `Spawning`
/// that had to come back as a reconnectable (Retryable) row rather than a
/// stuck one. Upstream has no typed state machine: a row carries only the
/// three-word `status`, and every restored agent is normalized to
/// Detached/Exited at boot. So the property that must survive the upgrade
/// is: a fork row whose `state_json` says Spawning, Retryable, Detached,
/// Created or Exited loads (the column is ignored, not a parse failure) with
/// a status that lets the user reconnect it, and the legacy `status` column
/// the fork kept writing is what decides it.
#[test]
fn fork_state_json_rows_load_as_reconnectable_sessions() {
    let (_tmp, path) = fork_main_database();
    let conn = Connection::open(&path).unwrap();
    for (id, handle, status, state_json) in [
        (
            "spawning-1",
            "spawning-1",
            "detached",
            r#"{"kind":"spawning","since":"2026-03-01T00:00:00Z"}"#,
        ),
        (
            "retryable-1",
            "retryable-1",
            "detached",
            r#"{"kind":"retryable","interrupted_at":"2026-03-01T00:00:00Z"}"#,
        ),
        (
            "exited-1",
            "exited-1",
            "exited",
            r#"{"kind":"exited","exit_code":137,"exited_at":"2026-03-01T00:00:00Z"}"#,
        ),
    ] {
        conn.execute(
            r#"insert into agent_sessions
                 (id, project_id, provider, source_branch, branch_name, worktree_path, title,
                  project_path, started_providers, status, created_at, updated_at, state_json,
                  session_settings, sort_order, shared_workspace, agent_handle, deleted_at,
                  provider_session_ids)
               values (?1, 'proj-1', 'claude', 'main', ?1, '/tmp/' || ?1, null, '/home/ada/widget',
                       '[]', ?3, '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z', ?4, null, 9, 0,
                       ?2, null, '{}')"#,
            rusqlite::params![id, handle, status, state_json],
        )
        .unwrap();
    }
    drop(conn);

    let store = SessionStore::open(&path).expect("open");
    let live = store.load_sessions().expect("load");
    let status_of = |id: &str| {
        live.iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id} lost in the upgrade"))
            .status
    };
    use dux_core::model::SessionStatus;
    assert_eq!(
        status_of("spawning-1"),
        SessionStatus::Detached,
        "an interrupted spawn must be reconnectable, not stuck"
    );
    assert_eq!(status_of("retryable-1"), SessionStatus::Detached);
    assert_eq!(status_of("exited-1"), SessionStatus::Exited);
}
