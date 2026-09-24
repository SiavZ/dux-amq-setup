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

/// Per-session settings exported to the provider: the fork's YOLO,
/// envelope-verify and custom system prompt switches
/// ([`SessionSettings::to_pty_env`](crate::session_settings::SessionSettings::to_pty_env)),
/// which the AMQ wrappers read to choose CLI flags.
///
/// Read from the store rather than the engine's in-memory map because the
/// create and reconnect jobs build the env on worker threads with only the
/// paths and config; the settings row is the persisted source of truth and
/// is written before any change is applied (persist-before-mutate), so the
/// two never disagree at launch. A store that cannot be read yields the
/// defaults with a warning rather than blocking the launch.
pub fn session_settings_env(
    paths: &DuxPaths,
    config: &Config,
    session: &AgentSession,
) -> Vec<(String, String)> {
    let settings = crate::storage::SessionStore::open(&paths.sessions_db_path)
        .and_then(|store| store.load_session_settings_for(&session.id))
        .unwrap_or_else(|err| {
            crate::logger::warn(&format!(
                "could not read session settings for {}; launching with defaults: {}",
                crate::sanitize::for_terminal(&session.id),
                crate::sanitize::for_terminal(&format!("{err:#}"))
            ));
            crate::session_settings::SessionSettings::default()
        });
    settings
        .to_pty_env(&session.provider, config.amq.inject.verify_envelope)
        .vars
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentWorkspace, FolderWorkspace, ProviderKind, SessionStatus};
    use chrono::Utc;

    fn session(dir: &std::path::Path) -> AgentSession {
        AgentSession {
            id: "s1".to_string(),
            // What the create job assigns for a folder named "My Agent"; the
            // launch exports the persisted handle, never re-derives it.
            agent_handle: "my-agent".to_string(),
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
                // Session settings: always exported so the bridge sees a
                // deterministic value (fork to_pty_env).
                "DUX_AMQ_VERIFY",
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

        // No DUX_* identity (two readers on one inbox), but the session's
        // settings still apply to every provider process it runs.
        assert_eq!(
            env,
            [
                ("DUX_AMQ_VERIFY".to_string(), "0".to_string()),
                ("KEY".to_string(), "v".to_string()),
            ]
        );
    }

    /// Stored per-session settings reach the launch env: YOLO maps to the
    /// provider's wrapper variable, the verify override wins over the global
    /// default, and a custom system prompt is exported. The fork applied these
    /// at every create and reconnect; without it the settings modal saved
    /// values no agent ever saw.
    #[test]
    fn stored_session_settings_reach_the_launch_env() {
        let dir = tempfile::tempdir().unwrap();
        let paths = paths(dir.path());
        std::fs::create_dir_all(&paths.root).unwrap();
        let session = session(dir.path());
        let store = crate::storage::SessionStore::open(&paths.sessions_db_path).unwrap();
        store.create_session(&session).unwrap();
        store
            .set_session_settings(
                &session.id,
                &crate::session_settings::SessionSettings {
                    yolo_permissions: true,
                    verify_envelope_override: Some(true),
                    system_prompt: Some("be terse".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();

        let env = agent_launch_env(&paths, &Config::default(), &session, "slot", Vec::new());
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

        assert_eq!(get("CLAUDE_AMQ_YOLO"), Some("1"));
        assert_eq!(get("DUX_AMQ_VERIFY"), Some("1"));
        assert_eq!(get("DUX_SYSTEM_PROMPT"), Some("be terse"));
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
