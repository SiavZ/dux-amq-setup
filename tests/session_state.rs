//! Integration tests for audit02 P1-Z (Phase 18) — explicit session
//! state machine.
//!
//! Phase 2 (this revision) embedded the [`PtyHandle`] inside the
//! `Live` and `Detached` variants. As a consequence, [`SessionState`]
//! is no longer `Clone`, `PartialEq`, `Serialize`, or `Deserialize`.
//! These tests pin:
//!
//! - the legal-transition matrix on the `Created`/`Spawning`/`Exited`
//!   path (the variants we can construct without a real PTY),
//! - the JSON round-trip via [`PersistedSessionState`] for those same
//!   variants, and
//! - the storage layer's read-back of the new `state_json` column.

use chrono::{Duration, Utc};
use dux::model::{AgentSession, PersistedSessionState, ProviderKind, SessionState};
use dux::storage::SessionStore;

/// Illegal transitions must be rejected loudly. We can't `Detach` a
/// brand-new session — there is no PTY to detach from. Phase 18's
/// whole point is to fail-fast on these instead of silently no-oping.
#[test]
fn illegal_transition_rejected() {
    let now = Utc::now();
    let state = SessionState::Created { created_at: now };

    let result = state.transition("detached", now);
    assert!(
        result.is_err(),
        "Created -> detached should be rejected, got {result:?}"
    );

    // Self-transitions are also rejected — they would mask genuine
    // duplicate-event bugs (e.g. two `on_spawn_succeeded` calls).
    let exited = SessionState::Exited {
        exit_code: Some(0),
        exited_at: now,
    };
    let self_result = exited.transition("exited", now);
    assert!(
        self_result.is_err(),
        "Exited -> exited self-transition should be rejected"
    );
}

/// Happy path through the PTY-less transitions:
/// `Created -> Spawning -> Exited -> Spawning`. The variants that
/// require a `PtyHandle` (`Live`, `Detached`) are exercised by the
/// runtime tests in `src/app/sessions.rs` where a real `echo` PTY is
/// spawned.
#[test]
fn valid_transitions_succeed() {
    let t0 = Utc::now();
    let t1 = t0 + Duration::seconds(1);
    let t2 = t0 + Duration::seconds(2);
    let t3 = t0 + Duration::seconds(3);

    let s = SessionState::Created { created_at: t0 };
    assert_eq!(s.name(), "created");

    let s = s.transition("spawning", t1).expect("created -> spawning");
    assert!(matches!(s, SessionState::Spawning { since } if since == t1));

    // Spawning -> Exited (spawn failure path).
    let s = s.transition("exited", t2).expect("spawning -> exited");
    assert!(matches!(s, SessionState::Exited { exited_at, .. } if exited_at == t2));

    // Exited -> Spawning (re-spawn after exit).
    let s = s.transition("spawning", t3).expect("exited -> spawning");
    assert!(matches!(s, SessionState::Spawning { since } if since == t3));
}

/// `SessionState::to_json` -> `from_json` must round-trip every
/// PTY-less persistable variant. `Live` and `Detached` carry a
/// `PtyHandle` and so cannot be constructed in this integration
/// test; they are covered by the in-process runtime tests.
#[test]
fn json_round_trip_for_persistable_variants() {
    let now = Utc::now();
    let cases = vec![
        SessionState::Created { created_at: now },
        SessionState::Spawning { since: now },
        SessionState::Exited {
            exit_code: Some(137),
            exited_at: now,
        },
        SessionState::Exited {
            exit_code: None,
            exited_at: now,
        },
    ];
    for state in cases {
        let json = state.to_json().expect("serialize");
        let back = SessionState::from_json(&json).expect("deserialize");
        // We can't `assert_eq!(state, back)` because `SessionState`
        // is no longer `PartialEq` (it owns process resources). Match
        // on the variant tag + key timestamp instead.
        match (&state, &back) {
            (SessionState::Created { created_at: a }, SessionState::Created { created_at: b }) => {
                assert_eq!(a, b)
            }
            (SessionState::Spawning { since: a }, SessionState::Spawning { since: b }) => {
                assert_eq!(a, b)
            }
            (
                SessionState::Exited {
                    exit_code: ea,
                    exited_at: ta,
                },
                SessionState::Exited {
                    exit_code: eb,
                    exited_at: tb,
                },
            ) => {
                assert_eq!(ea, eb);
                assert_eq!(ta, tb);
            }
            (a, b) => panic!("round-trip mismatch: {a:?} -> {b:?}"),
        }
    }
}

/// `Detached` carries a `PtyHandle` after Phase 18 phase 2; persist +
/// reload must collapse it (the handle cannot survive the process
/// restart). We assert this through `PersistedSessionState::from`
/// directly since constructing `Detached` from outside the crate
/// requires a `PtyHandle`. The mirror property — that
/// `PersistedSessionState::Detached` reloads as
/// `SessionState::Created` — is the persistence contract we test
/// here.
#[test]
fn persisted_detached_reloads_as_created() {
    let now = Utc::now();
    let persisted = PersistedSessionState::Detached { detached_at: now };
    let reloaded: SessionState = persisted.into();
    assert!(
        matches!(reloaded, SessionState::Created { created_at } if created_at == now),
        "PersistedSessionState::Detached must reload as Created (PtyHandle cannot survive restart), got {reloaded:?}"
    );
}

/// Migration 0002 adds the `state_json` column. After
/// `SessionStore::open` runs migrations on a fresh DB, that column
/// must exist on `agent_sessions`. The schema version must also be
/// at least 2.
#[test]
fn migration_0002_adds_state_json_column() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("phase18.sqlite3");

    let store = SessionStore::open(&path).expect("open fresh DB");
    let user_version: u32 = store
        .conn()
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .expect("read user_version");
    assert!(
        user_version >= 2,
        "expected user_version >= 2 after Phase 18 migration, got {user_version}"
    );

    let columns: Vec<String> = store
        .conn()
        .prepare("pragma table_info(agent_sessions)")
        .expect("prepare")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect");
    assert!(
        columns.iter().any(|c| c == "state_json"),
        "agent_sessions must have a state_json column after migration 0002; got {columns:?}"
    );
}

/// End-to-end: an `AgentSession` written through `upsert_session`
/// should populate `state_json`, and re-loading the session should
/// yield the same `SessionState` shape (`Exited` here — `Live` /
/// `Detached` need a real PTY handle that we don't seed in this
/// integration test).
#[test]
fn session_state_persists_round_trip_through_store() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("rt.sqlite3");
    let store = SessionStore::open(&path).expect("open");

    let now = Utc::now();
    let session = AgentSession {
        id: "rt-1".to_string(),
        project_id: "p".to_string(),
        project_path: None,
        provider: ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: "feat/phase-18".to_string(),
        worktree_path: "/tmp/rt-1".to_string(),
        title: None,
        started_providers: Vec::new(),
        state: SessionState::Exited {
            exit_code: Some(0),
            exited_at: now,
        },
        created_at: now,
        updated_at: now,
    };
    store.upsert_session(&session).expect("upsert");

    // Verify the column was populated with JSON we can parse back.
    let raw_json: Option<String> = store
        .conn()
        .query_row(
            "select state_json from agent_sessions where id = ?1",
            rusqlite::params!["rt-1"],
            |row| row.get(0),
        )
        .expect("select state_json");
    let raw_json = raw_json.expect("state_json should be populated by upsert");
    let parsed = SessionState::from_json(&raw_json).expect("parse JSON");
    assert!(
        matches!(parsed, SessionState::Exited { .. }),
        "expected Exited after Exited upsert, got {parsed:?}"
    );

    // Reload via the public API and confirm the state survives.
    let loaded = store.load_sessions().expect("load");
    let row = loaded.iter().find(|s| s.id == "rt-1").expect("loaded row");
    assert!(matches!(row.state, SessionState::Exited { .. }));
}
