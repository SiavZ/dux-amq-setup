//! Keeps every test away from the developer's real agent CLIs.
//!
//! A test that drives agent creation or a launch with the stock provider table
//! would otherwise exec the real `claude` (or `codex`, `opencode`, `copilot`)
//! found on the developer's `PATH`, in whatever directory the test happened to
//! point it at, including the home directory. Two halves stop that:
//!
//! - [`defuse_providers`] and [`harmless_config`] give the shared test fixtures a
//!   provider table whose names are the stock ones but whose command is a plain
//!   `cat` behind `sh`, so a test that needs "the claude provider" keeps the name
//!   and launches nothing real.
//! - [`refuse_real_agent_cli`] is called from the PTY spawn chokepoint in test
//!   builds only, and refuses to exec any command whose file name is a known agent
//!   CLI. A fixture that forgets the first half fails its launch instead of
//!   starting a real agent.
//!
//! Compiled only for dux-core's own tests and for the `test-support` feature that
//! `dux-tui` and `dux-web` enable as a dev-dependency, so none of it ships.

use crate::config::{Config, ProviderCommandConfig, default_provider_commands};

/// The file names of the agent CLIs a test must never exec. The stock providers
/// plus `gemini`, the documented example of a user-added one.
pub const REAL_AGENT_CLIS: &[&str] = &["claude", "codex", "opencode", "copilot", "gemini"];

/// The stand-in every defused provider runs. `sh -c 'exec cat'` rather than a bare
/// `cat` because a launch appends the provider's resume arguments (`--continue`,
/// `resume --last`) after `args`: `sh` takes them as positional parameters the
/// script never reads, where `cat` would try to open them and exit at once.
pub const HARMLESS_PROVIDER_COMMAND: &str = "sh";

/// The arguments that go with [`HARMLESS_PROVIDER_COMMAND`]. The trailing word is
/// `$0`, so a `ps` listing names what the process is.
pub fn harmless_provider_args() -> Vec<String> {
    vec![
        "-c".to_string(),
        "exec cat".to_string(),
        "dux-test-provider".to_string(),
    ]
}

/// Whether `command` names a real agent CLI, by its file name, so `claude`,
/// `/usr/local/bin/claude` and `~/.local/bin/claude` all count.
pub fn names_real_agent_cli(command: &str) -> bool {
    let name = std::path::Path::new(command.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    REAL_AGENT_CLIS.contains(&name)
}

/// The refusal the PTY spawn path returns in test builds. An `Err` rather than a
/// panic because launches run on worker threads, where a panic would vanish and
/// leave the test waiting; the ordinary spawn-failure path reports it instead.
pub fn refuse_real_agent_cli(command: &str) -> anyhow::Result<()> {
    if names_real_agent_cli(command) {
        let message = format!(
            "test guard: refused to spawn the real agent CLI '{command}'. A test resolved a \
             provider to a real agent binary; build its config with \
             dux_core::test_provider::harmless_config() or defuse_providers()."
        );
        eprintln!("{message}");
        anyhow::bail!(message);
    }
    Ok(())
}

/// Point every provider in `config` at the harmless stand-in, keeping each
/// provider's name, resume arguments and paste form. Every stock provider is
/// (re)inserted first, because a provider missing from the table resolves to a
/// command equal to its own name, which for `claude` is the real CLI.
pub fn defuse_providers(config: &mut Config) {
    for (name, stock) in default_provider_commands() {
        config
            .providers
            .commands
            .entry(name.to_string())
            .or_insert(stock);
    }
    for provider in config.providers.commands.values_mut() {
        defuse_provider(provider);
    }
}

fn defuse_provider(provider: &mut ProviderCommandConfig) {
    provider.command = HARMLESS_PROVIDER_COMMAND.to_string();
    provider.args = harmless_provider_args();
}

/// `Config::default()` with every provider defused. The config every shared test
/// fixture starts from.
pub fn harmless_config() -> Config {
    let mut config = Config::default();
    defuse_providers(&mut config);
    config
}

/// Asserts a fixture's config can launch no real agent CLI: not for any
/// stock provider, and not for its default provider.
pub fn assert_fixture_config_is_harmless(config: &Config) {
    let mut providers: Vec<crate::model::ProviderKind> = default_provider_commands()
        .iter()
        .map(|(name, _)| crate::model::ProviderKind::from_str(name))
        .collect();
    providers.push(config.default_provider());
    for provider in providers {
        let command = crate::config::provider_config(config, &provider).command;
        assert!(
            !names_real_agent_cli(&command),
            "the fixture resolves provider '{}' to the real agent CLI '{command}'",
            provider.as_str()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::provider_config;
    use crate::model::ProviderKind;

    #[test]
    fn a_real_agent_cli_is_recognised_by_its_file_name() {
        for name in REAL_AGENT_CLIS {
            assert!(names_real_agent_cli(name));
            assert!(names_real_agent_cli(&format!("/usr/local/bin/{name}")));
        }
        assert!(!names_real_agent_cli("cat"));
        assert!(!names_real_agent_cli("sh"));
        assert!(!names_real_agent_cli("/bin/claude-notes"));
    }

    #[test]
    fn the_spawn_guard_refuses_a_real_agent_cli_and_passes_the_stand_in() {
        let err = refuse_real_agent_cli("claude").unwrap_err();
        assert!(err.to_string().contains("test guard"), "{err}");
        refuse_real_agent_cli(HARMLESS_PROVIDER_COMMAND).unwrap();
    }

    #[test]
    fn the_pty_spawn_path_refuses_a_real_agent_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let err =
            match crate::pty::PtyClient::spawn_with_env("claude", &[], tmp.path(), 24, 80, 10, &[])
            {
                Ok(_) => panic!("the guard let a real agent CLI spawn"),
                Err(err) => err,
            };
        assert!(err.to_string().contains("test guard"), "{err}");
    }

    #[test]
    fn a_harmless_config_resolves_every_provider_to_the_stand_in() {
        let config = harmless_config();
        for (name, _) in default_provider_commands() {
            let resolved = provider_config(&config, &ProviderKind::from_str(name));
            assert_eq!(resolved.command, HARMLESS_PROVIDER_COMMAND, "{name}");
            // The stock name keeps its resume support, so resume paths still run.
            assert_eq!(
                resolved.supports_session_resume(),
                name != "copilot",
                "{name}"
            );
        }
        assert!(!names_real_agent_cli(
            &provider_config(&config, &config.default_provider()).command
        ));
    }

    #[test]
    fn the_shared_engine_fixture_cannot_launch_a_real_agent_cli() {
        let (engine, _tmp) = crate::engine::test_support::test_engine();
        assert_fixture_config_is_harmless(&engine.config);
    }

    #[test]
    fn the_stand_in_survives_resume_arguments() {
        let config = harmless_config();
        let resolved = provider_config(&config, &ProviderKind::from_str("codex"));
        let tmp = tempfile::tempdir().unwrap();
        let mut client = crate::pty::PtyClient::spawn_with_env(
            &resolved.command,
            &resolved.interactive_args(true),
            tmp.path(),
            24,
            80,
            10,
            &[],
        )
        .expect("spawn the stand-in");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(
            client.try_wait().is_none(),
            "the stand-in exited instead of staying up like an agent"
        );
    }
}
