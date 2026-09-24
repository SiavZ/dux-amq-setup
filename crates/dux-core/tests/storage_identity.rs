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
