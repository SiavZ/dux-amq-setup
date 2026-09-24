//! End-to-end checks of `dux peer` through the real binary, with DUX_HOME and
//! the AMQ root pointed at a temp dir so the host's real state is untouched.

use std::path::Path;
use std::process::{Command, Output};

use dux_core::model::{AgentSession, AgentWorkspace, FolderWorkspace, ProviderKind, SessionStatus};
use dux_core::storage::SessionStore;

fn dux(home: &Path, amq: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dux"))
        .args(args)
        .env("DUX_HOME", home)
        .env("AMQ_GLOBAL_ROOT", amq)
        .env_remove("AM_ROOT")
        .env_remove("DUX_SESSION_ID")
        .env_remove("DUX_AMQ_HANDLE")
        .env_remove("AM_ME")
        // No broker on this port, so Claude Peers is reliably "unavailable".
        .env("CLAUDE_PEERS_PORT", "9")
        .output()
        .expect("run dux")
}

fn session(id: &str, provider: &str, folder: &Path, status: SessionStatus) -> AgentSession {
    AgentSession {
        id: id.to_string(),
        agent_handle: dux_core::model::normalize_agent_handle(id),
        shared_workspace: false,
        deleted_at: None,
        slot_tab_id: format!("{id}-slot"),
        provider: ProviderKind::new(provider),
        workspace: AgentWorkspace::Folder(FolderWorkspace {
            folder_path: folder.display().to_string(),
        }),
        title: None,
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: true,
        status,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_focused_tab: None,
    }
}

#[test]
fn peer_help_list_and_sync_run_through_the_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let amq = tmp.path().join("amq");
    std::fs::create_dir_all(&home).unwrap();
    let live = tmp.path().join("Live Agent");
    let gone = tmp.path().join("gone-agent");
    let exited = tmp.path().join("exited-agent");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::create_dir_all(&exited).unwrap();
    let store = SessionStore::open(&home.join("sessions.sqlite3")).unwrap();
    store
        .upsert_session(&session("s1", "claude", &live, SessionStatus::Active))
        .unwrap();
    store
        .upsert_session(&session("s2", "codex", &gone, SessionStatus::Active))
        .unwrap();
    store
        .upsert_session(&session("s3", "codex", &exited, SessionStatus::Exited))
        .unwrap();

    let help = dux(&home, &amq, &["peer", "--help"]);
    assert!(help.status.success());
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("dux peer send [--from <handle>] [--transport auto|amq|claude-peers]"));

    let sync = dux(&home, &amq, &["peer", "sync-amq"]);
    assert!(sync.status.success(), "{sync:?}");
    let sync = String::from_utf8_lossy(&sync.stdout);
    assert!(sync.contains("configured agents added: 3"), "{sync}");
    assert!(amq.join("agents/live-agent/.dux-amq-source").is_file());

    let list = dux(&home, &amq, &["peer", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(list.contains("live-agent"), "{list}");
    // Unroutable sessions are hidden: a missing directory, an exited agent.
    assert!(!list.contains("gone-agent"), "{list}");
    assert!(!list.contains("exited-agent"), "{list}");
    assert!(list.contains("Claude Peers broker: unavailable"), "{list}");
}

#[test]
fn peer_send_refuses_unknown_targets_and_requires_the_broker_for_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let amq = tmp.path().join("amq");
    std::fs::create_dir_all(&home).unwrap();
    let live = tmp.path().join("worker");
    std::fs::create_dir_all(&live).unwrap();
    SessionStore::open(&home.join("sessions.sqlite3"))
        .unwrap()
        .upsert_session(&session("s1", "claude", &live, SessionStatus::Active))
        .unwrap();

    let unknown = dux(&home, &amq, &["peer", "send", "nobody", "hi"]);
    assert!(!unknown.status.success());
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("does not exist or is not reachable")
    );

    // A Claude target defaults to Claude Peers and fails loudly without it,
    // naming the manual AMQ override.
    let claude = dux(
        &home,
        &amq,
        &["peer", "send", "worker", "--status", "please"],
    );
    assert!(!claude.status.success());
    let err = String::from_utf8_lossy(&claude.stderr);
    assert!(err.contains("require Claude Peers"), "{err}");
    assert!(err.contains("--transport amq"), "{err}");
}
