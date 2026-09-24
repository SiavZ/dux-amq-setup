//! A `sessions.sqlite3` written by the fork (SiavZ/dux-amq-setup `main`,
//! numbered migrations up to `PRAGMA user_version = 6`) must open under this
//! build, keep every row, and keep each agent's `session_settings` blob:
//! that column is where the fork stored context mode, YOLO and the system
//! prompt, and losing it would silently demote every Orchestrator/Worker to
//! Attended on the first launch after the switch.

use dux_core::session_settings::{ContextMode, SessionSettings};
use dux_core::storage::SessionStore;
use rusqlite::Connection;

/// The fork's schema at `user_version = 6`, transcribed from its migrations
/// `0005_shared_workspace.sql` (final `agent_sessions` / `session_prs` shape)
/// and `0006_provider_session_ids.sql`. Projects lived in config.toml there,
/// so there is no `projects` table.
const FORK_SCHEMA_V6: &str = r#"
create table agent_sessions (
    id text primary key,
    project_id text not null,
    provider text not null,
    source_branch text not null,
    branch_name text not null,
    worktree_path text not null,
    title text,
    project_path text,
    started_providers text not null default '[]',
    status text not null,
    created_at text not null,
    updated_at text not null,
    state_json text,
    session_settings text,
    sort_order integer not null default 0,
    shared_workspace integer not null default 0,
    agent_handle text not null unique,
    deleted_at text,
    provider_session_ids text not null default '{}'
);
create table session_prs (
    session_id text not null,
    pr_number integer not null,
    owner_repo text not null,
    state text not null default 'OPEN',
    title text not null default '',
    primary key (session_id, pr_number),
    foreign key (session_id) references agent_sessions(id) on delete cascade
);
create index idx_agent_sessions_sort_order
    on agent_sessions(sort_order, updated_at desc, id);
pragma user_version = 6;

insert into agent_sessions
  (id, project_id, provider, source_branch, branch_name, worktree_path, title,
   status, created_at, updated_at, session_settings, sort_order, agent_handle)
values
  ('orch', 'p1', 'codex', 'main', 'orchestrator', '/wt/orch', 'Lead',
   'detached', '2026-07-01T00:00:00Z', '2026-07-02T00:00:00Z',
   '{"mode":"orchestrator","yolo_permissions":true,"system_prompt":"stay on target"}',
   0, 'orch'),
  ('work', 'p1', 'claude', 'main', 'feature/x', '/wt/work', null,
   'detached', '2026-07-01T00:00:00Z', '2026-07-03T00:00:00Z',
   '{"mode":"worker","auto_clear_on_task_done":true,"watch_rule_arm":{"2":false}}',
   1, 'work'),
  ('plain', 'p1', 'claude', 'main', 'plain', '/wt/plain', null,
   'detached', '2026-07-01T00:00:00Z', '2026-07-04T00:00:00Z',
   null, 2, 'plain'),
  ('broken', 'p1', 'claude', 'main', 'broken', '/wt/broken', null,
   'detached', '2026-07-01T00:00:00Z', '2026-07-05T00:00:00Z',
   '{not json', 3, 'broken');
"#;

fn fork_database() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sessions.sqlite3");
    let conn = Connection::open(&path).expect("open");
    conn.execute_batch(FORK_SCHEMA_V6).expect("fork schema");
    drop(conn);
    (tmp, path)
}

#[test]
fn a_fork_schema_6_database_opens_and_keeps_every_session() {
    let (_tmp, path) = fork_database();
    let store = SessionStore::open(&path).expect("fork database opens");
    let mut ids: Vec<String> = store
        .load_sessions()
        .expect("sessions load")
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    assert_eq!(ids, ["broken", "orch", "plain", "work"]);
}

#[test]
fn fork_session_settings_survive_the_switch() {
    let (_tmp, path) = fork_database();
    let store = SessionStore::open(&path).expect("open");
    let settings = store.load_session_settings().expect("settings load");

    let orch = &settings["orch"];
    assert_eq!(orch.mode, ContextMode::Orchestrator);
    assert!(orch.yolo_permissions);
    assert_eq!(orch.system_prompt.as_deref(), Some("stay on target"));

    let work = &settings["work"];
    assert_eq!(work.mode, ContextMode::Worker);
    assert!(work.auto_clear_on_task_done);
    assert_eq!(work.watch_rule_arm.get(&2), Some(&false));

    // NULL and malformed blobs read as the inert default (never stored).
    assert!(!settings.contains_key("plain"));
    assert!(!settings.contains_key("broken"));
}

#[test]
fn reopening_is_a_no_op_and_writes_round_trip() {
    let (_tmp, path) = fork_database();
    {
        let store = SessionStore::open(&path).expect("first open");
        store
            .set_session_settings(
                "plain",
                &SessionSettings {
                    mode: ContextMode::Worker,
                    ..SessionSettings::default()
                },
            )
            .expect("write");
        // Back to default clears the column.
        store
            .set_session_settings("work", &SessionSettings::default())
            .expect("reset");
    }
    let store = SessionStore::open(&path).expect("second open");
    let settings = store.load_session_settings().expect("load");
    assert_eq!(settings["plain"].mode, ContextMode::Worker);
    assert!(!settings.contains_key("work"));
    assert_eq!(settings["orch"].mode, ContextMode::Orchestrator);
}

#[test]
fn setting_an_unknown_session_is_an_error_not_a_silent_no_op() {
    let (_tmp, path) = fork_database();
    let store = SessionStore::open(&path).expect("open");
    assert!(
        store
            .set_session_settings("ghost", &SessionSettings::default())
            .is_err()
    );
}
