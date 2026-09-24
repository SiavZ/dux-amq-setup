//! `agent_sessions.session_settings` round-trips through SQLite, and every
//! degenerate value a real database can hold (the fork's NULLs, a corrupted
//! blob, the new `{}` default) reads as the inert default. Ported from the
//! fork's `tests/storage_integration.rs` / `tests/storage_migrations.rs`
//! (audit03 Phase 01). The fork kept settings on `AgentSession`; here they are
//! a side map read with `SessionStore::load_session_settings`.

use chrono::Utc;
use dux_core::model::{
    AgentSession, AgentWorkspace, BranchProvenance, ManagedWorkspace, ProviderKind, SessionStatus,
};
use dux_core::session_settings::{ContextMode, SessionSettings};
use dux_core::storage::SessionStore;
use rusqlite::Connection;

fn fixture_session(id: &str) -> AgentSession {
    let now = Utc::now();
    AgentSession {
        id: id.to_string(),
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

fn store_with(id: &str) -> (tempfile::TempDir, std::path::PathBuf, SessionStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sessions.sqlite3");
    let store = SessionStore::open(&path).expect("open");
    store
        .create_session(&fixture_session(id))
        .expect("create session");
    (dir, path, store)
}

fn settings_of(store: &SessionStore, id: &str) -> SessionSettings {
    store
        .load_session_settings()
        .expect("load settings")
        .get(id)
        .cloned()
        .unwrap_or_default()
}

fn raw_column(path: &std::path::Path, id: &str) -> Option<String> {
    Connection::open(path)
        .expect("raw open")
        .query_row(
            "select session_settings from agent_sessions where id = ?1",
            [id],
            |row| row.get(0),
        )
        .expect("read raw column")
}

#[test]
fn session_settings_default_round_trip_through_sqlite() {
    let (_dir, path, store) = store_with("rt-default");
    assert_eq!(
        settings_of(&store, "rt-default"),
        SessionSettings::default()
    );

    // A new row is never NULL, and what is on disk is a JSON object, so a
    // later version can read it. Writing the default back keeps it an object.
    for _ in 0..2 {
        let raw = raw_column(&path, "rt-default").expect("column is not null");
        let parsed: serde_json::Value = serde_json::from_str(&raw).expect("on-disk json parses");
        assert!(parsed.is_object(), "expected a JSON object, got {raw}");
        store
            .set_session_settings("rt-default", &SessionSettings::default())
            .expect("write default");
    }
}

#[test]
fn session_settings_default_when_blob_malformed() {
    let (_dir, path, store) = store_with("bad-row");
    Connection::open(&path)
        .unwrap()
        .execute(
            "update agent_sessions set session_settings = '{not json' where id = 'bad-row'",
            [],
        )
        .expect("write garbage");
    assert_eq!(settings_of(&store, "bad-row"), SessionSettings::default());
    // A malformed blob must not stop the session itself from loading.
    assert!(
        store
            .load_sessions()
            .unwrap()
            .iter()
            .any(|s| s.id == "bad-row")
    );
}

/// New databases declare the column `not null`, but a fork database (schema
/// 0003+) declared it nullable and holds NULLs. Recreate that shape.
#[test]
fn session_settings_default_when_column_null() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("null.sqlite3");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "create table agent_sessions (
                id text primary key, project_id text not null, provider text not null,
                source_branch text not null, branch_name text not null,
                worktree_path text not null, title text, project_path text,
                started_providers text not null default '[]', status text not null,
                created_at text not null, updated_at text not null,
                session_settings text
             );
             insert into agent_sessions (id, project_id, provider, source_branch,
                branch_name, worktree_path, status, created_at, updated_at, session_settings)
             values ('null-row', 'p', 'claude', 'main', 'b', '/tmp/null-row', 'detached',
                '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', null);",
        )
        .unwrap();
    }
    let store = SessionStore::open(&path).expect("open");
    assert_eq!(raw_column(&path, "null-row"), None, "NULL is kept as-is");
    assert_eq!(settings_of(&store, "null-row"), SessionSettings::default());
}

#[test]
fn session_settings_full_payload_round_trip_through_sqlite() {
    let (_dir, path, store) = store_with("rt-full");
    let full = SessionSettings {
        mode: ContextMode::Worker,
        yolo_permissions: true,
        watch_rule_arm: [(0usize, false), (2, true)].into_iter().collect(),
        auto_clear_on_task_done: true,
        verify_envelope_override: Some(true),
        system_prompt: Some("You are a senior reviewer. Be thorough but kind.".to_string()),
    };
    store.set_session_settings("rt-full", &full).expect("write");
    drop(store);
    let store = SessionStore::open(&path).expect("reopen");
    assert_eq!(settings_of(&store, "rt-full"), full);

    // A status upsert must never clobber the settings blob.
    let mut session = fixture_session("rt-full");
    session.status = SessionStatus::Active;
    store.upsert_session(&session).expect("upsert");
    assert_eq!(settings_of(&store, "rt-full"), full);
}

/// A database from before the settings column existed (the fork's v2, and
/// upstream's own pre-AMQ databases) gains it on open without disturbing rows.
#[test]
fn migrate_v2_to_v3_adds_session_settings_column() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("v2.sqlite3");
    let store = SessionStore::open(&path).expect("create");
    store
        .create_session(&fixture_session("old"))
        .expect("create session");
    drop(store);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("alter table agent_sessions drop column session_settings;")
            .expect("drop column to recreate the pre-settings shape");
    }
    let columns = |path: &std::path::Path| -> Vec<(String, bool, Option<String>)> {
        Connection::open(path)
            .unwrap()
            .prepare("pragma table_info(agent_sessions)")
            .unwrap()
            .query_map([], |row| Ok((row.get(1)?, row.get(3)?, row.get(4)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert!(!columns(&path).iter().any(|c| c.0 == "session_settings"));

    let store = SessionStore::open(&path).expect("migrating open");
    let col = columns(&path)
        .into_iter()
        .find(|c| c.0 == "session_settings")
        .expect("column added");
    assert!(col.1, "declared not null");
    assert_eq!(col.2.as_deref(), Some("'{}'"));
    assert_eq!(raw_column(&path, "old").as_deref(), Some("{}"));
    assert_eq!(settings_of(&store, "old"), SessionSettings::default());
    assert!(store.load_sessions().unwrap().iter().any(|s| s.id == "old"));
}
