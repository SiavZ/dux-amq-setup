//! Provider-agnostic watch rules.
//!
//! A "watch rule" pairs a regex against an action to take when that regex
//! matches the agent's terminal output. The motivating use case is auto-
//! retrying Claude Code when Anthropic's server-side capacity throttle
//! surfaces ("API Error: Server is temporarily limiting requests …"), but
//! nothing in the engine is Claude-specific, rules are loaded from each
//! provider's config and the engine simply matches strings and dispatches
//! actions. New providers (codex, gemini, opencode, custom) can ship their
//! own rules with no engine changes.
//!
//! Threat model: regex evaluation runs against output that may be partially
//! attacker-controlled (project files the agent reads, AI responses, child
//! process noise). The engine uses the linear-time `regex` crate with a
//! per-pattern `size_limit` cap to avoid pathological compilation, and bounds
//! per-rule actions with a budget by default. `budget.max_attempts = 0` is an
//! explicit unlimited opt-in for trusted rules. Rules ship commented out in
//! the rendered config (opt-in), are capped at 32 per provider, and can be
//! disarmed by hand from the watch-rules palette command.

pub mod builtin;
pub mod delivery;
pub mod engine;
pub mod reset_time;
pub mod rule;
pub mod runtime;

pub use engine::{RuleSnapshot, RuleStateKind, WatchEffect, WatchEngine};
pub use rule::{WaitFormat, WatchAction, WatchBackoff, WatchBudget, WatchRule, WatchRuleKind};
