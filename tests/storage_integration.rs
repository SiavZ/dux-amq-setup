//! Integration tests for SQLite hardening, immutable session identity, and
//! soft-delete tombstones.
//!
//! These tests live under `tests/` (rather than as `#[cfg(test)]` modules in
//! `storage.rs`) so they exercise `dux::storage::SessionStore` through the
//! library's public surface — the same way an external consumer would.

use chrono::Utc;
use dux::model::{AgentSession, ProviderKind, SessionSettings, SessionState};
use dux::storage::SessionStore;

fn fixture_session(id: &str) -> AgentSession {
    let now = Utc::now();
    AgentSession {
        id: id.to_string(),
        project_id: "proj".to_string(),
        project_path: None,
        provider: ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: format!("branch-{id}"),
        worktree_path: format!("/tmp/{id}"),
        agent_handle: dux::model::normalize_agent_handle(id),
        shared_workspace: false,
        deleted_at: None,
        title: None,
        started_providers: Vec::new(),
        state: SessionState::Created { created_at: now },
        settings: SessionSettings::default(),
        created_at: now,
        updated_at: now,
    }
}

/// PRAGMAs in `SessionStore::open` must put SQLite in WAL journaling mode.
/// WAL is the foundation of the audit02 P1-W hardening: it allows the
/// online backup API to copy the database without blocking writers and
/// keeps readers and writers from blocking each other under contention.
#[test]
fn open_sets_wal_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("test.sqlite3");

    let storage = SessionStore::open(&path).expect("open SessionStore");
    let conn = storage.conn();
    let mode: String = conn
        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
        .expect("query journal_mode");
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "expected WAL journal mode, got {mode}"
    );

    // Foreign keys must also be on (it's part of the same PRAGMA batch).
    let fk: i64 = conn
        .query_row("PRAGMA foreign_keys;", [], |r| r.get(0))
        .expect("query foreign_keys");
    assert_eq!(fk, 1, "expected foreign_keys = 1, got {fk}");
}

/// A corrupted on-disk database must produce a clean `Err` rather than a
/// panic. Operators rely on this to know when to restore from `.bak`; a
/// panic in the middle of `App::new` would leave dux in a half-initialized
/// state.
#[test]
fn integrity_check_failure_returns_error_not_panic() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("corrupt.sqlite3");

    // Write enough bytes to look like *something* to SQLite but not a valid
    // database. SQLite refuses to open a file whose magic header isn't the
    // canonical "SQLite format 3\0", so this triggers the error path inside
    // `Connection::open` (or the integrity check, depending on the platform).
    std::fs::write(
        &path,
        b"not a sqlite db, just some bytes that are not sqlite",
    )
    .expect("write corrupt file");

    let result = SessionStore::open(&path);
    assert!(
        result.is_err(),
        "expected SessionStore::open on a corrupt file to return Err"
    );
    let err = result.err().unwrap();
    let msg = format!("{err:#}");
    // The error chain should mention the path so operators can find it.
    assert!(
        msg.contains("corrupt.sqlite3"),
        "expected error message to mention the corrupt file path; got: {msg}"
    );
}

/// `backup_to` must use SQLite's online backup API to produce a destination
/// file that is itself a valid, openable SQLite database. This is the
/// mechanism the periodic backup worker relies on for spot-VM resilience.
#[test]
fn backup_to_produces_valid_db() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let src_path = tmp.path().join("src.sqlite3");
    let dst_path = tmp.path().join("dst.sqlite3");

    let storage = SessionStore::open(&src_path).expect("open src");
    storage
        .backup_to(&dst_path)
        .expect("backup_to succeeds on a fresh DB");

    assert!(dst_path.exists(), "backup destination not created");
    let dst_meta = std::fs::metadata(&dst_path).expect("stat dst");
    assert!(dst_meta.len() > 0, "backup destination is empty");

    // Re-opening the backup through the same hardened path must succeed
    // (so its journal_mode, integrity, and migration are all healthy).
    let restored = SessionStore::open(&dst_path).expect("open backup");
    let mode: String = restored
        .conn()
        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
        .expect("query journal_mode on restored DB");
    assert_eq!(mode.to_lowercase(), "wal");
}

/// audit03 Phase 01: a session whose `settings` is the default value
/// must round-trip through sqlite without surprises.
#[test]
fn session_settings_default_round_trip_through_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(dir.path().join("rt.sqlite3").as_path()).expect("open");

    let session = fixture_session("rt-default");
    store.upsert_session(&session).expect("upsert");

    let loaded = store.load_sessions().expect("load");
    let s = loaded
        .iter()
        .find(|s| s.id == "rt-default")
        .expect("session present");
    assert_eq!(s.settings, SessionSettings::default());

    // The on-disk JSON must be valid JSON (so future readers across
    // versions can deserialise it). Don't assert on a specific string
    // shape — adding a field to `SessionSettings` would break a
    // brittle equality check, which is exactly the forward-compat
    // contract we're trying to preserve.
    let raw_json: Option<String> = store
        .conn()
        .query_row(
            "select session_settings from agent_sessions where id = 'rt-default'",
            [],
            |row| row.get(0),
        )
        .expect("read raw json");
    let raw = raw_json.expect("session_settings should be NOT NULL after upsert");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("on-disk json must parse");
    assert!(
        parsed.is_object(),
        "session_settings must serialize to a JSON object, got {raw}"
    );
}

/// audit03 Phase 01 / Phase 2: a session whose `settings` carries
/// non-default values for every typed knob must round-trip through
/// sqlite preserving each one. This is the canary that
/// `serde(default)` on individual fields hasn't been mis-applied to
/// also strip data on serialize.
#[test]
fn session_settings_full_payload_round_trip_through_sqlite() {
    use dux::model::ContextMode;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(dir.path().join("full.sqlite3").as_path()).expect("open");

    let mut session = fixture_session("rt-full");
    session.settings = SessionSettings {
        mode: ContextMode::Worker,
        yolo_permissions: true,
        watch_rule_arm: [(0usize, false), (2, true)].into_iter().collect(),
        auto_clear_on_task_done: true,
        verify_envelope_override: Some(true),
        system_prompt: Some("You are a senior reviewer. Be thorough but kind.".to_string()),
    };
    store.upsert_session(&session).expect("upsert");

    let loaded = store.load_sessions().expect("load");
    let s = loaded
        .iter()
        .find(|s| s.id == "rt-full")
        .expect("session present");
    assert_eq!(s.settings, session.settings);
}

/// audit03 Phase 01 asymmetric-default policy: a row whose
/// `session_settings` column is NULL must load as
/// `SessionSettings::default()` without any warning. Mirrors what an
/// older dux binary would write when it doesn't know about the
/// column.
#[test]
fn session_settings_default_when_column_null() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("null.sqlite3");
    let store = SessionStore::open(&path).expect("open");

    // Insert a normal row, then NULL out the column directly. Doing it
    // this way (rather than constructing an INSERT by hand) means we
    // exercise the real upsert path and only override the column we
    // care about.
    let session = fixture_session("null-row");
    store.upsert_session(&session).expect("upsert");
    store
        .conn()
        .execute(
            "update agent_sessions set session_settings = NULL where id = 'null-row'",
            [],
        )
        .expect("null out");

    let loaded = store.load_sessions().expect("load");
    let s = loaded
        .iter()
        .find(|s| s.id == "null-row")
        .expect("session present");
    assert_eq!(
        s.settings,
        SessionSettings::default(),
        "NULL session_settings must load as default()"
    );
}

/// audit03 Phase 01 asymmetric-default policy: a row whose
/// `session_settings` blob is unparseable JSON must load as
/// `SessionSettings::default()` (and emit a warn-level log, which the
/// test suite doesn't assert on directly because we don't install a
/// tracing subscriber here — the behavioural contract is the value).
#[test]
fn session_settings_default_when_blob_malformed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bad.sqlite3");
    let store = SessionStore::open(&path).expect("open");

    let session = fixture_session("bad-row");
    store.upsert_session(&session).expect("upsert");
    store
        .conn()
        .execute(
            "update agent_sessions set session_settings = '{not json' where id = 'bad-row'",
            [],
        )
        .expect("write garbage");

    let loaded = store.load_sessions().expect("load");
    let s = loaded
        .iter()
        .find(|s| s.id == "bad-row")
        .expect("session present");
    assert_eq!(
        s.settings,
        SessionSettings::default(),
        "malformed session_settings must load as default()"
    );
}

#[test]
fn session_identity_and_workspace_mode_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(&dir.path().join("identity.sqlite3")).expect("open");
    let mut session = fixture_session("identity");
    session.agent_handle = "durable-handle".to_string();
    session.shared_workspace = true;
    store.upsert_session(&session).expect("upsert");

    let loaded = store.load_sessions().expect("load");
    assert_eq!(loaded[0].agent_handle(), "durable-handle");
    assert!(loaded[0].shared_workspace());

    let mut changed = loaded[0].clone();
    changed.agent_handle = "changed-handle".to_string();
    let err = store
        .upsert_session(&changed)
        .expect_err("handle mutation must fail");
    assert!(format!("{err:#}").contains("immutable agent handle"));
}

#[test]
fn session_creation_normalizes_and_suffixes_local_handle_collisions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(&dir.path().join("create-handle.sqlite3")).expect("open");

    let mut first = fixture_session("first");
    first.agent_handle = "Same Handle".to_string();
    store
        .assign_unique_agent_handle(&mut first)
        .expect("assign first handle");
    store.upsert_session(&first).expect("insert first");
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

#[test]
fn load_sessions_fails_closed_on_invalid_handle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(&dir.path().join("invalid.sqlite3")).expect("open");
    store
        .upsert_session(&fixture_session("invalid"))
        .expect("upsert");
    store
        .conn()
        .execute_batch(
            r#"
            pragma ignore_check_constraints = on;
            update agent_sessions set agent_handle = 'Bad/handle' where id = 'invalid';
            pragma ignore_check_constraints = off;
            "#,
        )
        .expect("inject invalid handle");

    let err = store
        .load_sessions()
        .expect_err("invalid handle must fail closed");
    let message = format!("{err:#}");
    assert!(message.contains("database corruption"));
    assert!(message.contains("invalid agent_handle"));
}

#[test]
fn load_sessions_fails_closed_on_duplicate_handle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("duplicate.sqlite3");
    let conn = rusqlite::Connection::open(&path).expect("open raw database");
    conn.execute_batch(
        r#"
        create table agent_sessions (
            id text, project_id text, provider text, source_branch text,
            branch_name text, worktree_path text, agent_handle text,
            shared_workspace integer, deleted_at text, title text,
            project_path text, started_providers text, status text,
            state_json text, session_settings text, sort_order integer,
            created_at text, updated_at text
        );
        insert into agent_sessions values
            ('a', 'p', 'claude', 'main', 'a', '/tmp/a', 'same', 0, null,
             null, null, '[]', 'detached', null, null, 0,
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'),
            ('b', 'p', 'claude', 'main', 'b', '/tmp/b', 'same', 0, null,
             null, null, '[]', 'detached', null, null, 1,
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
        "#,
    )
    .expect("seed duplicate handles");
    drop(conn);

    let store = SessionStore::open_read_only(&path).expect("open corrupt schema read-only");
    let err = store
        .load_sessions()
        .expect_err("duplicate handles must fail closed");
    assert!(format!("{err:#}").contains("duplicate agent_handle"));
}

#[test]
fn soft_delete_hides_active_session_and_retains_tombstone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SessionStore::open(&dir.path().join("soft-delete.sqlite3")).expect("open");
    let mut session = fixture_session("soft-delete");
    session.project_path = Some("/tmp/project".to_string());
    store.upsert_session(&session).expect("upsert");

    store
        .soft_delete_session(&session.id)
        .expect("soft delete session");
    let raw_deleted_at: String = store
        .conn()
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
    assert_eq!(tombstones[0].id, session.id);
    assert_eq!(tombstones[0].project_id, session.project_id);
    assert_eq!(tombstones[0].project_path, session.project_path);
    assert_eq!(tombstones[0].provider, session.provider);
    assert_eq!(tombstones[0].agent_handle(), session.agent_handle());
    assert_eq!(tombstones[0].worktree_path, session.worktree_path);
    assert!(tombstones[0].deleted_at.is_some());

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
