//! The environment an agent provider is launched with, composed in one place
//! so every launch path layers it the same way:
//!
//! 1. Yaran identity (`YARAN_SESSION_ID`, `YARAN_STORE_ID`, `YARAN_PROVIDER`,
//!    `YARAN_AMQ_HANDLE`), which the AMQ wrappers and `yaran peer send` read.
//! 2. Per-session settings variables (see [`session_settings_env`]).
//! 3. The user's resolved `[env]` (global plus project), LAST, so an
//!    explicit user setting always wins, matching upstream's rule that the
//!    caller env overrides everything the spawn sets itself.

use crate::config::{Config, YaranPaths};
use crate::model::AgentSession;
use crate::session_settings::SessionSettings;

/// Compose the launch environment for `session`'s tab `tab_id`.
///
/// Only the session-slot tab carries the Yaran identity. An extra tab is a
/// second provider process in the same session; exporting the same
/// `YARAN_AMQ_HANDLE` to it would put two readers on one AMQ inbox, so it
/// launches with the settings and user variables only, as it did before.
pub fn agent_launch_env(
    paths: &YaranPaths,
    config: &Config,
    session: &AgentSession,
    settings: &SessionSettings,
    tab_id: &str,
    user_env: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut env = Vec::new();
    if tab_id == session.slot_tab_id().as_str() {
        env.extend(crate::peer::launch_env_for_session(paths, session));
    }
    env.extend(session_settings_env(config, session, settings));
    env.extend(runtime_paths_env(paths, config));
    env.extend(with_legacy_env_aliases(user_env));
    env
}

pub(crate) fn runtime_paths_env(paths: &YaranPaths, config: &Config) -> Vec<(String, String)> {
    let mut env = vec![(
        "YARAN_HOME".to_string(),
        paths.root.to_string_lossy().into_owned(),
    )];
    if let Some(queue) = crate::amq::queue::resolve_queue_dir(&config.amq.inject) {
        env.push((
            "YARAN_AMQ_QUEUE_DIR".to_string(),
            queue.to_string_lossy().into_owned(),
        ));
    }
    with_legacy_env_aliases(env)
}

/// Per-session settings exported to the provider: the fork's YOLO,
/// envelope-verify and custom system prompt switches
/// ([`SessionSettings::to_pty_env`]), which the AMQ wrappers read to choose
/// CLI flags.
///
/// Pure: the caller passes the settings it already holds (the engine's
/// in-memory map, which is loaded at restore and written only after the row
/// persists). Launch paths must never open a second store connection: every
/// `SessionStore::open` runs the migration, whose orphan-tab sweep can delete
/// a tab row another connection has just inserted.
pub fn session_settings_env(
    config: &Config,
    session: &AgentSession,
    settings: &SessionSettings,
) -> Vec<(String, String)> {
    settings
        .to_pty_env(&session.provider, config.amq.inject.verify_envelope)
        .vars
}

/// Normalize one env layer before composing it with another. An explicit new
/// spelling wins within the layer, while a legacy user override still beats
/// generated identity because the user layer is applied last.
pub(crate) fn with_legacy_env_aliases(mut env: Vec<(String, String)>) -> Vec<(String, String)> {
    let original = env.clone();
    for (name, value) in &original {
        let (new, old) = if let Some(suffix) = name.strip_prefix("YARAN_") {
            (name.clone(), format!("DUX_{suffix}"))
        } else if let Some(suffix) = name.strip_prefix("DUX_") {
            (format!("YARAN_{suffix}"), name.clone())
        } else {
            continue;
        };
        let resolved = original
            .iter()
            .rev()
            .find(|(key, _)| key == &new)
            .map(|(_, value)| value)
            .unwrap_or(value);
        for key in [new, old] {
            if let Some((_, value)) = env.iter_mut().find(|(name, _)| name == &key) {
                *value = resolved.clone();
            } else {
                env.push((key, resolved.clone()));
            }
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentWorkspace, FolderWorkspace, ProviderKind, SessionStatus};
    use chrono::Utc;

    #[test]
    fn new_env_spelling_wins_conflicts_and_legacy_only_still_exports_both() {
        for vars in [
            vec![("DUX_SYSTEM_PROMPT".into(), "old".into())],
            vec![("YARAN_SYSTEM_PROMPT".into(), "old".into())],
            vec![
                ("DUX_SYSTEM_PROMPT".into(), "ignored".into()),
                ("YARAN_SYSTEM_PROMPT".into(), "old".into()),
            ],
        ] {
            let env = with_legacy_env_aliases(vars);
            for key in ["YARAN_SYSTEM_PROMPT", "DUX_SYSTEM_PROMPT"] {
                assert_eq!(env.iter().find(|(name, _)| name == key).unwrap().1, "old");
            }
        }
    }

    #[test]
    fn legacy_user_overrides_reach_both_child_spellings_after_generated_identity() {
        let dir = tempfile::tempdir().unwrap();
        let env = agent_launch_env(
            &paths(dir.path()),
            &Config::default(),
            &session(dir.path()),
            &SessionSettings::default(),
            "slot",
            vec![("DUX_AMQ_HANDLE".into(), "user-override".into())],
        );
        for key in ["YARAN_AMQ_HANDLE", "DUX_AMQ_HANDLE"] {
            assert_eq!(
                env.iter().rev().find(|(name, _)| name == key).unwrap().1,
                "user-override"
            );
        }
        for suffix in [
            "SESSION_ID",
            "STORE_ID",
            "PROVIDER",
            "HOME",
            "AMQ_QUEUE_DIR",
        ] {
            let value = |prefix| {
                env.iter()
                    .find(|(name, _)| name == &format!("{prefix}{suffix}"))
                    .unwrap()
                    .1
                    .clone()
            };
            assert_eq!(value("YARAN_"), value("DUX_"));
        }
    }

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

    fn paths(dir: &std::path::Path) -> YaranPaths {
        let root = dir.join("home");
        YaranPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("yaran.lock"),
            root,
        }
    }

    #[test]
    fn slot_tab_gets_identity_first_and_user_env_last() {
        let dir = tempfile::tempdir().unwrap();
        let user = vec![("YARAN_AMQ_HANDLE".to_string(), "user-override".to_string())];

        let env = agent_launch_env(
            &paths(dir.path()),
            &Config::default(),
            &session(dir.path()),
            &SessionSettings::default(),
            "slot",
            user,
        );

        let keys = env
            .iter()
            .filter(|(key, _)| !key.starts_with("DUX_"))
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "YARAN_SESSION_ID",
                "YARAN_STORE_ID",
                "YARAN_PROVIDER",
                "YARAN_AMQ_HANDLE",
                // Session settings: always exported so the bridge sees a
                // deterministic value (fork to_pty_env).
                "YARAN_AMQ_VERIFY",
                "YARAN_HOME",
                "YARAN_AMQ_QUEUE_DIR",
                "YARAN_AMQ_HANDLE"
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
            &SessionSettings::default(),
            "extra-tab",
            user.clone(),
        );

        // No YARAN_* identity (two readers on one inbox), but the session's
        // settings still apply to every provider process it runs.
        let env = env
            .into_iter()
            .filter(|(key, _)| !key.ends_with("HOME") && !key.ends_with("AMQ_QUEUE_DIR"))
            .collect::<Vec<_>>();
        assert_eq!(
            env,
            [
                ("YARAN_AMQ_VERIFY".to_string(), "0".to_string()),
                ("DUX_AMQ_VERIFY".to_string(), "0".to_string()),
                ("KEY".to_string(), "v".to_string()),
            ]
        );
    }

    /// A session's settings reach the launch env: YOLO maps to the provider's
    /// wrapper variable, the verify override wins over the global default,
    /// and a custom system prompt is exported. The fork applied these at every
    /// create and reconnect; without it the settings modal saved values no
    /// agent ever saw.
    #[test]
    fn session_settings_reach_the_launch_env() {
        let dir = tempfile::tempdir().unwrap();
        let settings = SessionSettings {
            yolo_permissions: true,
            verify_envelope_override: Some(true),
            system_prompt: Some("be terse".to_string()),
            ..Default::default()
        };

        let env = agent_launch_env(
            &paths(dir.path()),
            &Config::default(),
            &session(dir.path()),
            &settings,
            "slot",
            Vec::new(),
        );
        let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());

        assert_eq!(get("CLAUDE_AMQ_YOLO"), Some("1"));
        assert_eq!(get("YARAN_AMQ_VERIFY"), Some("1"));
        assert_eq!(get("YARAN_SYSTEM_PROMPT"), Some("be terse"));
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
                status_message: Default::default(),
            },
        );
        let handle = request
            .env
            .iter()
            .find(|(k, _)| k == "YARAN_AMQ_HANDLE")
            .map(|(_, v)| v.clone())
            .expect("slot-tab launch exports YARAN_AMQ_HANDLE");

        let args = vec![
            "-c".to_string(),
            "printf 'H=%s S=%s P=%s' \"$YARAN_AMQ_HANDLE\" \"$YARAN_SESSION_ID\" \"$YARAN_PROVIDER\""
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
