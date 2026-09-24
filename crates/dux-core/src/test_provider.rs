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
//! - [`refuse_unlisted_spawn`] is called from the PTY spawn chokepoint in test
//!   builds only, and refuses to exec any command that is not one of the
//!   [`ALLOWED_TEST_COMMANDS`]. Any CLI can be a provider, so a list of the agent
//!   CLIs we know about could never be complete; the allowlist names the few
//!   harmless programs tests genuinely run instead. A fixture that forgets the
//!   first half fails its launch instead of starting a real agent.
//!
//! Compiled only for dux-core's own tests and for the `test-support` feature that
//! `dux-tui` and `dux-web` enable as a dev-dependency, so none of it ships.

use crate::config::{Config, ProviderCommandConfig, default_provider_commands};

/// The file names of the only programs a test may spawn through a PTY. Matched
/// exactly against the command's file name, so `/bin/sh` is allowed and `Claude`
/// is not (on a case-insensitive filesystem that name is the real `claude`).
///
/// The shells are here because a terminal's default command is `$SHELL`, which
/// differs from one developer's machine to the next; the rest are the stand-ins
/// and one-shot helpers the suites use. To allow another program, add its file
/// name here, and only if running it can touch nothing outside the test's own
/// scratch directory.
pub const ALLOWED_TEST_COMMANDS: &[&str] = &[
    "sh", "bash", "dash", "zsh", "fish", "cat", "sleep", "true", "false", "printf", "echo", "env",
];

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

/// Whether a test may spawn `command`, judged by its file name, so `sh` and
/// `/bin/sh` both count and `claude`, `Cat` and `/bin/claude-notes` do not.
pub fn is_allowed_test_command(command: &str) -> bool {
    let name = std::path::Path::new(command.trim())
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    ALLOWED_TEST_COMMANDS.contains(&name)
}

/// The refusal the PTY spawn path returns in test builds. An `Err` rather than a
/// panic because launches run on worker threads, where a panic would vanish and
/// leave the test waiting; the ordinary spawn-failure path reports it instead.
pub fn refuse_unlisted_spawn(command: &str) -> anyhow::Result<()> {
    if !is_allowed_test_command(command) {
        let message = format!(
            "test guard: refused to spawn '{command}', which is not one of the programs tests \
             may run. If a test resolved a provider to a real agent CLI, build its config with \
             dux_core::test_provider::harmless_config() or defuse_providers(). If the program is \
             a harmless helper the test genuinely needs, add its file name to \
             dux_core::test_provider::ALLOWED_TEST_COMMANDS."
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
            is_allowed_test_command(&command),
            "the fixture resolves provider '{}' to '{command}', which is not a harmless stand-in",
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
    fn an_allowed_command_is_recognised_by_its_exact_file_name() {
        for name in ALLOWED_TEST_COMMANDS {
            assert!(is_allowed_test_command(name));
            assert!(is_allowed_test_command(&format!("/usr/bin/{name}")));
        }
        assert!(!is_allowed_test_command("claude"));
        assert!(!is_allowed_test_command("Cat"));
        assert!(!is_allowed_test_command("/bin/claude-notes"));
        assert!(!is_allowed_test_command(""));
    }

    #[test]
    fn the_spawn_guard_refuses_a_real_agent_cli_and_passes_the_stand_in() {
        let err = refuse_unlisted_spawn("claude").unwrap_err();
        assert!(err.to_string().contains("test guard"), "{err}");
        refuse_unlisted_spawn(HARMLESS_PROVIDER_COMMAND).unwrap();
    }

    /// Any CLI can be a provider, so a list of the agents we know about cannot
    /// keep a test away from the one we do not: only the stand-ins are allowed.
    #[test]
    fn the_spawn_guard_refuses_any_command_that_is_not_a_listed_stand_in() {
        for command in [
            "claude",
            "/usr/local/bin/codex",
            "aider",
            "goose",
            "cursor-agent",
        ] {
            let err = refuse_unlisted_spawn(command).unwrap_err();
            assert!(
                err.to_string().starts_with("test guard:"),
                "{command}: {err}"
            );
        }
    }

    /// A case-insensitive filesystem resolves `Claude` to the real `claude`, so
    /// the comparison is exact: a differently cased name is simply not listed.
    #[test]
    fn the_spawn_guard_refuses_a_differently_cased_agent_cli() {
        for command in ["Claude", "CLAUDE", "/opt/bin/Codex", "Sh"] {
            let err = refuse_unlisted_spawn(command).unwrap_err();
            assert!(
                err.to_string().starts_with("test guard:"),
                "{command}: {err}"
            );
        }
    }

    #[test]
    fn the_spawn_guard_error_says_how_to_extend_the_allowlist() {
        let err = refuse_unlisted_spawn("aider").unwrap_err().to_string();
        assert!(err.contains("aider"), "{err}");
        assert!(err.contains("ALLOWED_TEST_COMMANDS"), "{err}");
    }

    #[test]
    fn the_pty_spawn_path_runs_an_allowed_stand_in_by_name_and_by_path() {
        let tmp = tempfile::tempdir().unwrap();
        for command in ["sh", "/bin/sh"] {
            let args = vec!["-c".to_string(), "exit 0".to_string()];
            crate::pty::PtyClient::spawn_with_env(command, &args, tmp.path(), 24, 80, 10, &[])
                .unwrap_or_else(|err| panic!("{command} was refused: {err}"));
        }
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
        assert!(is_allowed_test_command(
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
