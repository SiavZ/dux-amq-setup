//! `[amq]` configuration: the inject-queue drainer (`[amq.inject]`) and the
//! Orchestrator watchdog (`[amq.orchestrator]`).
//!
//! Every field is rendered with its documentation in the TUI's canonical
//! config template (`dux-tui/src/config.rs`, `config_schema`). Keep the
//! comments there in step with the doc comments here.

use serde::{Deserialize, Serialize};

/// Configuration for the AMQ companion (file-based message bus).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmqConfig {
    pub inject: AmqInjectConfig,
    pub orchestrator: AmqOrchestratorConfig,
}

/// Tunables for the dux-side drainer that consumes
/// `~/.local/share/dux-amq/inject-queue/<receiver>/*.msg` files written by
/// `dux-amq-inject-bridge`. The drainer types each body into the matching
/// agent's PTY only when the agent is idle, fixing the "stuck in input field"
/// failure where a provider TUI drops a trailing Enter received mid-stream.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmqInjectConfig {
    /// Master switch. When `false`, no queue files are consumed. Default true.
    pub enabled: bool,
    /// Override the queue root. Empty means
    /// `~/.local/share/dux-amq/inject-queue` (`XDG_DATA_HOME`-aware).
    pub queue_dir: String,
    /// Bottom-of-screen substrings meaning the agent is busy. Plain
    /// case-sensitive literals, no regex.
    pub busy_markers: Vec<String>,
    /// How many recent PTY rows are scanned for busy markers. Default 5.
    pub busy_scan_lines: usize,
    /// Warn once a queued message has waited this many seconds. The file
    /// stays on disk. Default 600. `0` disables the warning.
    pub delivery_timeout_secs: u64,
    /// Maximum age of a wake file before it is moved to `.expired/` instead
    /// of being injected. Applies at startup (plain and crash-left inflight
    /// files) and to held in-memory messages. Default 0 (disabled).
    pub max_message_age_secs: u64,
    /// Polling fallback interval for filesystems where `notify` is lossy.
    /// Default 5000 ms; floored at 100 ms.
    pub poll_interval_ms: u64,
    /// Reject queue files larger than this. Default 65536.
    pub max_message_bytes: u64,
    /// Strict HMAC verification of wake envelopes by the bridge. Exported to
    /// agents as `DUX_AMQ_VERIFY`. Default false (single-user trust model).
    pub verify_envelope: bool,
    /// Quiet window for the active-session hold: while the operator has the
    /// target agent focused, delivery waits until they have not typed for
    /// this many seconds. `0` always holds; ten years or more always
    /// delivers. Default 60.
    pub active_session_quiet_secs: u64,
    /// Minimum gap between typing the body (phase 1) and sending Enter
    /// (phase 2). Non-zero values are floored to 250 ms. `0` is a debugging
    /// escape hatch. Default 250.
    pub phase_delay_ms: u64,
    /// Delay delivery for this long after dux starts, so restored provider
    /// TUIs finish booting. Default 10000.
    pub startup_grace_ms: u64,
    /// Minimum quiet period after one wake is submitted before the next may
    /// be injected into the same session. Default 10000.
    pub post_delivery_cooldown_ms: u64,
    /// Suppress Worker auto-clear while AMQ collaboration is recent (see
    /// [`crate::amq::activity`]). Default 1800. `0` disables only the
    /// recent-activity window; pending mail still blocks auto-clear.
    pub auto_clear_collaboration_quiet_secs: u64,
}

impl Default for AmqInjectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            queue_dir: String::new(),
            busy_markers: vec![
                "esc to interrupt".to_string(),
                "ctrl+c to interrupt".to_string(),
            ],
            busy_scan_lines: 5,
            delivery_timeout_secs: 600,
            max_message_age_secs: 0,
            poll_interval_ms: 5_000,
            max_message_bytes: 65_536,
            verify_envelope: false,
            active_session_quiet_secs: 60,
            phase_delay_ms: 250,
            startup_grace_ms: 10_000,
            post_delivery_cooldown_ms: 10_000,
            auto_clear_collaboration_quiet_secs: 1_800,
        }
    }
}

/// Tunables for the Orchestrator watchdog: periodically wake Orchestrator
/// sessions with a checkpoint prompt so they poll same-project workers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmqOrchestratorConfig {
    /// Master switch. Only sessions marked Orchestrator are affected.
    /// Default true.
    pub enabled: bool,
    /// Seconds between checkpoint nudges per project while a same-project
    /// worker exists. `0` disables periodic nudges but keeps the launch-time
    /// policy prompt. Default 900.
    pub poll_interval_secs: u64,
    /// Replacement checkpoint text. Non-blank (after trim) is injected
    /// verbatim instead of the built-in template. Default empty.
    pub checkpoint_prompt: String,
}

impl Default for AmqOrchestratorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 900,
            checkpoint_prompt: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amq_inject_defaults_match_constants() {
        let c = AmqConfig::default();
        assert!(c.inject.enabled);
        assert!(c.inject.queue_dir.is_empty());
        assert!(
            c.inject
                .busy_markers
                .iter()
                .any(|m| m == "esc to interrupt"),
            "default busy markers should include Claude Code's footer"
        );
        assert_eq!(c.inject.busy_scan_lines, 5);
        assert_eq!(c.inject.delivery_timeout_secs, 600);
        assert_eq!(c.inject.max_message_age_secs, 0);
        assert_eq!(c.inject.poll_interval_ms, 5_000);
        assert_eq!(c.inject.max_message_bytes, 65_536);
        assert!(!c.inject.verify_envelope);
        assert_eq!(c.inject.active_session_quiet_secs, 60);
        assert_eq!(c.inject.phase_delay_ms, 250);
        assert_eq!(c.inject.startup_grace_ms, 10_000);
        assert_eq!(c.inject.post_delivery_cooldown_ms, 10_000);
        assert_eq!(c.inject.auto_clear_collaboration_quiet_secs, 1_800);
        assert!(c.orchestrator.enabled);
        assert_eq!(c.orchestrator.poll_interval_secs, 900);
        assert!(c.orchestrator.checkpoint_prompt.is_empty());
    }

    #[test]
    fn partial_toml_fills_defaults() {
        let c: crate::config::Config = toml::from_str(
            "[amq.inject]\nphase_delay_ms = 500\n[amq.orchestrator]\ncheckpoint_prompt = \"hi\"\n",
        )
        .unwrap();
        assert_eq!(c.amq.inject.phase_delay_ms, 500);
        assert_eq!(c.amq.inject.busy_scan_lines, 5);
        assert_eq!(c.amq.orchestrator.checkpoint_prompt, "hi");
        assert_eq!(c.amq.orchestrator.poll_interval_secs, 900);
    }
}
