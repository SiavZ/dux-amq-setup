//! The environment an agent provider is launched with, composed in one place
//! so every launch path layers it the same way:
//!
//! 1. Dux identity (`DUX_SESSION_ID`, `DUX_STORE_ID`, `DUX_PROVIDER`,
//!    `DUX_AMQ_HANDLE`), which the AMQ wrappers and `dux peer send` read.
//! 2. Per-session settings variables (see [`session_settings_env`]).
//! 3. The user's resolved `[env]` (global plus project), LAST, so an
//!    explicit user setting always wins, matching upstream's rule that the
//!    caller env overrides everything the spawn sets itself.

use crate::config::{Config, DuxPaths};
use crate::model::AgentSession;

/// Compose the launch environment for `session`'s tab `tab_id`.
///
/// Only the session-slot tab carries the Dux identity. An extra tab is a
/// second provider process in the same session; exporting the same
/// `DUX_AMQ_HANDLE` to it would put two readers on one AMQ inbox, so it
/// launches with the settings and user variables only, as it did before.
pub fn agent_launch_env(
    paths: &DuxPaths,
    config: &Config,
    session: &AgentSession,
    tab_id: &str,
    user_env: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if tab_id == session.slot_tab_id().as_str() {
        env.extend(crate::peer::launch_env_for_session(paths, session));
    }
    env.extend(session_settings_env(paths, config, session));
    env.extend(user_env);
    env
}

/// Per-session settings exported to the provider (the fork's YOLO and AMQ
/// envelope-verify switches).
// INTEGRATION: extend with Engine::session_settings(id).to_pty_env() (maple)
pub fn session_settings_env(
    _paths: &DuxPaths,
    _config: &Config,
    _session: &AgentSession,
) -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentWorkspace, FolderWorkspace, ProviderKind, SessionStatus};
    use chrono::Utc;

    fn session(dir: &std::path::Path) -> AgentSession {
        AgentSession {
            id: "s1".to_string(),
            agent_handle: "s1".to_string(),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: "slot".to_string(),
            provider: ProviderKind::new("claude"),
            workspace: AgentWorkspace::Folder(FolderWorkspace {
                folder_path: dir.join("My Agent").display().to_string(),
            }),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_focused_tab: None,
        }
    }

    fn paths(dir: &std::path::Path) -> DuxPaths {
        let root = dir.join("home");
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        }
    }

    #[test]
    fn slot_tab_gets_identity_first_and_user_env_last() {
        let dir = tempfile::tempdir().unwrap();
        let user = vec![("DUX_AMQ_HANDLE".to_string(), "user-override".to_string())];

        let env = agent_launch_env(
            &paths(dir.path()),
            &Config::default(),
            &session(dir.path()),
            "slot",
            user,
        );

        let keys = env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "DUX_SESSION_ID",
                "DUX_STORE_ID",
                "DUX_PROVIDER",
                "DUX_AMQ_HANDLE",
                "DUX_AMQ_HANDLE"
            ]
        );
        assert_eq!(env[3].1, "my-agent");
        // The user's own `[env]` is applied last, so it wins at spawn.
        assert_eq!(env.last().unwrap().1, "user-override");
    }

    #[test]
    fn an_extra_tab_does_not_share_the_sessions_amq_identity() {
        let dir = tempfile::tempdir().unwrap();
        let user = vec![("KEY".to_string(), "v".to_string())];

        let env = agent_launch_env(
            &paths(dir.path()),
            &Config::default(),
            &session(dir.path()),
            "extra-tab",
            user.clone(),
        );

        assert_eq!(env, user);
    }

    /// The engine's launch builder (the path every reconnect, reopen and
    /// resume takes) carries the identity, and a real PTY spawned with that
    /// request's env hands it to the child process.
    #[test]
    fn engine_launch_requests_carry_the_identity_into_a_real_pty() {
        let (engine, _tmp) = crate::engine::test_support::test_engine();
        let folder = tempfile::tempdir().unwrap();
        let session = crate::engine::test_support::sample_standalone_session(
            "s1",
            &folder.path().display().to_string(),
        );

        let request = engine.build_agent_launch_request(
            session,
            false,
            (5, 80),
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: String::new(),
            },
        );
        let handle = request
            .env
            .iter()
            .find(|(k, _)| k == "DUX_AMQ_HANDLE")
            .map(|(_, v)| v.clone())
            .expect("slot-tab launch exports DUX_AMQ_HANDLE");

        let args = vec![
            "-c".to_string(),
            "printf 'H=%s S=%s P=%s' \"$DUX_AMQ_HANDLE\" \"$DUX_SESSION_ID\" \"$DUX_PROVIDER\""
                .to_string(),
        ];
        let client = crate::pty::PtyClient::spawn_with_env(
            "/bin/sh",
            &args,
            folder.path(),
            5,
            80,
            100,
            &request.env,
        )
        .unwrap();
        let want = format!("H={handle} S=s1 P=claude");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let snapshot = client.snapshot();
            let text = snapshot
                .cells
                .iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>();
            if text.contains(&want) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "child never printed {want:?}; saw {text:?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
