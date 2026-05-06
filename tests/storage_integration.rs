//! Integration tests for the SQLite session store hardening landed in
//! audit02 phase 14 (P1-W).
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
/// must round-trip through sqlite without surprises. The default form
/// has no fields (yet) but its JSON shape is fixed (`{}`) so we can
/// observe it on disk too.
#[test]
fn session_settings_round_trip_through_sqlite() {
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

    // Confirm the on-disk JSON is the canonical default shape.
    let raw_json: Option<String> = store
        .conn()
        .query_row(
            "select session_settings from agent_sessions where id = 'rt-default'",
            [],
            |row| row.get(0),
        )
        .expect("read raw json");
    assert_eq!(raw_json.as_deref(), Some("{}"));
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
