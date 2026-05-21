//! Built-in watch rules.
//!
//! These rules are constructed in code rather than loaded from
//! `config.toml` so the operator cannot accidentally edit them away.
//! Today the only built-in is the auto-clear-after-task-done rule
//! used by [`crate::model::ContextMode::Worker`] sessions when
//! `SessionSettings.auto_clear_on_task_done` is true. A worker
//! that emits the `[task-done]` sentinel at end-of-task gets its
//! context cleared by the engine on the next tick.
//!
//! Why not config.toml? Two reasons:
//! 1. The rule is feature infrastructure, not user policy. If the
//!    operator removes it the Worker mode loses half its meaning.
//! 2. The clear command (`/clear`, `/new`, …) is provider-specific
//!    and the rule needs to pick it dynamically. Hardcoding it in
//!    config would either bake in claude semantics for every
//!    provider or require operator-driven duplication.
//!
//! See `docs/plans/audits/audit03/01-session-settings-modal.md` §6.

use crate::model::ProviderKind;

use super::rule::{WatchAction, WatchBackoff, WatchBudget, WatchRule, WatchRuleKind};

/// Effectively unlimited for a long-running worker session, while still
/// keeping the engine's generic finite-budget model.
pub const AUTO_CLEAR_MAX_ATTEMPTS: u32 = 10_000;
pub const AUTO_CLEAR_LABEL: &str = "auto-clear after task done";

/// Default sentinel the orchestrator instructs Worker-mode agents to
/// emit at end-of-task. Lowercased, square-bracketed, no whitespace.
/// The dux-side AMQ drainer also appends a postscript note to wake
/// payloads asking the agent to use this exact token; see
/// `crate::app::inject_runtime` for the producer side.
///
/// Allow-deadcode because the constant becomes a real cross-module
/// link in audit03 Phase 5 (postscript injection). It's exposed as
/// the public token name so consumers don't grep for the magic
/// string in two places.
#[allow(dead_code)]
pub const TASK_DONE_SENTINEL: &str = "[task-done]";

/// Build the built-in auto-clear-on-task-done rule for a session
/// whose provider's "wipe context" command is `clear_command`. The
/// rule can fire repeatedly across tasks, but has a long cooldown so
/// a duplicate sentinel emitted while the clear is still in flight
/// doesn't race-fire.
///
/// Pattern: literal `[task-done]` (regex-escaped to avoid the
/// brackets being interpreted as a character class).
pub fn auto_clear_rule_for(clear_command: &str) -> WatchRule {
    WatchRule {
        // `regex::escape` would also work, but the sentinel is
        // a fixed literal so the manually escaped form is fine and
        // makes the intent obvious to readers.
        pattern: r"\[task-done\]".to_string(),
        label: AUTO_CLEAR_LABEL.to_string(),
        action: WatchAction::SendText {
            text: clear_command.to_string(),
            append_enter: true,
        },
        backoff: WatchBackoff {
            // Short backoff — by the time the sentinel is visible
            // the agent has already finished writing its reply, and
            // we want the clear to land before the operator
            // tab-switches into the session. Two seconds gives the
            // PTY a chance to fully render the final tokens before
            // we type a slash command on top.
            initial_ms: 2_000,
            max_ms: 10_000,
            multiplier: 2.0,
            jitter_ms: 500,
        },
        // Auto-clear must keep working for every AMQ task in a long
        // worker session. The engine's baseline/cooldown logic handles
        // stale visible sentinels, so this budget is only a hard guard
        // against runaway behavior, not a per-task limit.
        budget: WatchBudget {
            max_attempts: AUTO_CLEAR_MAX_ATTEMPTS,
        },
        cooldown_ms: 60_000,
        kind: WatchRuleKind::BuiltInAutoClear,
    }
}

/// The "wipe my context" slash command for each known provider. New
/// providers default to `/clear` because it's the most common
/// convention; if a provider rejects it the rule still fires (the
/// agent prints a help message) but no context is actually cleared,
/// which is the safer failure mode than typing something destructive.
pub fn provider_clear_command(provider: &ProviderKind) -> &'static str {
    match provider.as_str() {
        "claude" => "/clear",
        "codex" => "/new",
        "gemini" => "/clear",
        "opencode" => "/clear",
        _ => "/clear",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_clear_command_known_providers() {
        assert_eq!(
            provider_clear_command(&ProviderKind::new("claude")),
            "/clear"
        );
        assert_eq!(provider_clear_command(&ProviderKind::new("codex")), "/new");
        assert_eq!(
            provider_clear_command(&ProviderKind::new("gemini")),
            "/clear"
        );
        assert_eq!(
            provider_clear_command(&ProviderKind::new("opencode")),
            "/clear"
        );
    }

    #[test]
    fn provider_clear_command_unknown_falls_back_to_clear() {
        assert_eq!(
            provider_clear_command(&ProviderKind::new("future-cli")),
            "/clear"
        );
    }

    #[test]
    fn auto_clear_rule_has_safe_defaults() {
        let r = auto_clear_rule_for("/clear");
        assert_eq!(r.pattern, r"\[task-done\]");
        assert_eq!(r.budget.max_attempts, AUTO_CLEAR_MAX_ATTEMPTS);
        match r.action {
            WatchAction::SendText { text, append_enter } => {
                assert_eq!(text, "/clear");
                assert!(append_enter);
            }
            _ => panic!("expected SendText action"),
        }
    }

    #[test]
    fn auto_clear_rule_pattern_matches_sentinel_literal() {
        // Build a regex with the same builder the engine uses and
        // confirm the escaped pattern matches the literal sentinel
        // but NOT a partial-bracketed substring.
        use regex::RegexBuilder;
        let r = auto_clear_rule_for("/clear");
        let re = RegexBuilder::new(&r.pattern)
            .build()
            .expect("rule pattern compiles");
        assert!(re.is_match("done: [task-done]"));
        assert!(re.is_match("[task-done]"));
        assert!(!re.is_match("task-done")); // no brackets, no match
        assert!(!re.is_match("[TASK-DONE]")); // case-sensitive by default
    }
}
