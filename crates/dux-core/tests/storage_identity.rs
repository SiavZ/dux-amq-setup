//! Fork `tests/storage_integration.rs` identity tests, ported under their
//! fork names where upstream's storage has the same behaviour.

use chrono::Utc;
use dux_core::model::{
    AgentSession, AgentWorkspace, BranchProvenance, ManagedWorkspace, ProviderKind, SessionStatus,
};
use dux_core::storage::SessionStore;

fn fixture_session(id: &str) -> AgentSession {
    let now = Utc::now();
    AgentSession {
        id: id.to_string(),
        agent_handle: id.to_string(),
        shared_workspace: false,
        deleted_at: None,
        slot_tab_id: format!("{id}-slot"),
        provider: ProviderKind::new("claude"),
        workspace: AgentWorkspace::Managed(ManagedWorkspace {
            project_id: "proj".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: format!("branch-{id}"),
            initial_branch: format!("branch-{id}"),
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
}

/// WAL is on. The fork also asserted `foreign_keys = 1`; upstream
/// deliberately leaves it off (its delete paths are written for that, see
/// `SessionStore::open`), so only the journal mode is pinned here.
#[test]
fn open_sets_wal_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("test.sqlite3");
    let _store = SessionStore::open(&path).expect("open SessionStore");
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    let mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
        .expect("query journal_mode");
    assert_eq!(mode.to_lowercase(), "wal");
}

/// A new agent's handle is normalized to the AMQ alphabet and suffixed past
/// every existing handle, tombstones included.
#[test]
fn session_creation_normalizes_and_suffixes_local_handle_collisions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(&dir.path().join("create-handle.sqlite3")).expect("open");

    let mut first = fixture_session("first");
    first.agent_handle = "Same Handle".to_string();
    store
        .assign_unique_agent_handle(&mut first)
        .expect("assign first handle");
    store.create_session(&first).expect("insert first");
    assert_eq!(first.agent_handle(), "same-handle");
    store
        .soft_delete_session(&first.id)
        .expect("tombstone first");

    let mut second = fixture_session("second");
    second.agent_handle = "Same Handle".to_string();
    store
        .assign_unique_agent_handle(&mut second)
        .expect("assign collision suffix");
    assert_eq!(second.agent_handle(), "same-handle-2");
}

/// Two rows sharing one handle would route two agents to one AMQ inbox. The
/// fork refused to LOAD such a database; upstream's partial unique index makes
/// the state unwritable in the first place, which is the stronger guarantee.
#[test]
fn load_sessions_fails_closed_on_duplicate_handle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("duplicate.sqlite3");
    let store = SessionStore::open(&path).expect("open");
    store
        .create_session(&fixture_session("a"))
        .expect("insert a");
    let mut b = fixture_session("b");
    b.agent_handle = "a".to_string();
    assert!(
        store.create_session(&b).is_err(),
        "a second row with the same handle must be refused"
    );
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    assert!(
        conn.execute(
            "update agent_sessions set agent_handle = 'a' where id = 'b'",
            [],
        )
        .map(|_| ())
        .and_then(|()| {
            conn.execute(
                "insert into agent_sessions (id, project_id, provider, source_branch, branch_name, worktree_path, status, created_at, updated_at, agent_handle) \
                 values ('c', 'p', 'claude', 'main', 'c', '/tmp/c', 'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'a')",
                [],
            )
        })
        .is_err(),
        "even a raw write cannot give two rows one handle"
    );
    let loaded = store.load_sessions().expect("load");
    assert_eq!(loaded.len(), 1);
}

/// Fork `tests/storage_integration.rs`. A corrupt database file must surface
/// as an error naming the file (operators restore from `<path>.bak`), never
/// a panic halfway through startup.
#[test]
fn integrity_check_failure_returns_error_not_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("corrupt.sqlite3");
    std::fs::write(
        &path,
        b"not a sqlite db, just some bytes that are not sqlite",
    )
    .expect("write corrupt file");

    let err = match SessionStore::open(&path) {
        Ok(_) => panic!("expected SessionStore::open on a corrupt file to return Err"),
        Err(err) => err,
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("corrupt.sqlite3"),
        "the error must name the corrupt file; got: {msg}"
    );
}

/// Fork `tests/storage_integration.rs`. A stored handle outside the AMQ
/// alphabet names a path component of an inbox on disk; loading it would let
/// a tampered row steer AMQ traffic. The load refuses the whole database.
/// (Upstream has no CHECK constraint to bypass, so the row is written raw.)
#[test]
fn load_sessions_fails_closed_on_invalid_handle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("invalid.sqlite3");
    let store = SessionStore::open(&path).expect("open");
    store
        .create_session(&fixture_session("invalid"))
        .expect("insert");
    rusqlite::Connection::open(&path)
        .expect("raw open")
        .execute(
            "update agent_sessions set agent_handle = 'Bad/handle' where id = 'invalid'",
            [],
        )
        .expect("inject invalid handle");

    let err = store
        .load_sessions()
        .expect_err("invalid handle must fail closed");
    let message = format!("{err:#}");
    assert!(message.contains("database corruption"), "{message}");
    assert!(message.contains("invalid agent_handle"), "{message}");
}

/// Fork `tests/storage_integration.rs`. The handle and the shared-workspace
/// flag persist, and a handle, once written, can never be changed by an
/// upsert: AMQ inboxes on disk are named after it.
///
/// The fork test was named for a per-session `workspace_mode`; in the fork
/// that was the `shared_workspace` column, which is what this round-trips.
#[test]
fn session_identity_and_workspace_mode_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(&dir.path().join("identity.sqlite3")).expect("open");
    let mut session = fixture_session("identity");
    session.agent_handle = "durable-handle".to_string();
    session.shared_workspace = true;
    store.create_session(&session).expect("insert");

    let loaded = store.load_sessions().expect("load");
    assert_eq!(loaded[0].agent_handle(), "durable-handle");
    assert!(loaded[0].shared_workspace());

    let mut changed = loaded[0].clone();
    changed.agent_handle = "changed-handle".to_string();
    let err = store
        .upsert_session(&changed)
        .expect_err("handle mutation must fail");
    assert!(
        format!("{err:#}").contains("immutable agent handle"),
        "{err:#}"
    );
    assert_eq!(
        store.load_sessions().expect("reload")[0].agent_handle(),
        "durable-handle"
    );
}

/// Fork `tests/storage_integration.rs`. A soft delete hides the agent from
/// the live list but keeps a tombstone carrying its identity (the handle
/// stays reserved, purge can still find its data), stamped with an RFC 3339
/// time; a hard delete then removes the tombstone too.
#[test]
fn soft_delete_hides_active_session_and_retains_tombstone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("soft-delete.sqlite3");
    let store = SessionStore::open(&path).expect("open");
    let mut session = fixture_session("soft-delete");
    if let AgentWorkspace::Managed(managed) = &mut session.workspace {
        managed.project_path = Some("/tmp/project".to_string());
    }
    store.create_session(&session).expect("insert");

    store
        .soft_delete_session(&session.id)
        .expect("soft delete session");
    let raw_deleted_at: String = rusqlite::Connection::open(&path)
        .expect("raw open")
        .query_row(
            "select deleted_at from agent_sessions where id = ?1",
            rusqlite::params![session.id],
            |row| row.get(0),
        )
        .expect("read tombstone timestamp");
    chrono::DateTime::parse_from_rfc3339(&raw_deleted_at).expect("deleted_at is RFC 3339");
    assert!(store.load_sessions().expect("load active").is_empty());
    let tombstones = store
        .load_sessions_including_deleted()
        .expect("load tombstones");
    assert_eq!(tombstones.len(), 1);
    let tombstone = &tombstones[0];
    assert_eq!(tombstone.id, session.id);
    assert_eq!(tombstone.project_id(), session.project_id());
    assert_eq!(tombstone.project_path(), Some("/tmp/project"));
    assert_eq!(tombstone.provider, session.provider);
    assert_eq!(tombstone.agent_handle(), session.agent_handle());
    assert_eq!(tombstone.directory(), session.directory());
    assert!(tombstone.is_deleted());

    store
        .delete_session(&session.id)
        .expect("hard delete tombstone");
    assert!(
        store
            .load_sessions_including_deleted()
            .expect("load after hard delete")
            .is_empty()
    );
}
