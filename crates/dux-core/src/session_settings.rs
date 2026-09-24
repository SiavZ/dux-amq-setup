//! Per-session settings: context mode, YOLO, AMQ verify override, watch-rule
//! arm overrides, auto-clear opt-in and a per-session system prompt.
//!
//! Persisted as one JSON blob in `agent_sessions.session_settings` so adding
//! a knob never needs a schema change: only this struct grows. Every field
//! defaults to "do nothing" / "operator-managed" semantics. The
//! asymmetric-risk policy: an empty, missing or corrupt blob must never
//! enable autonomous behaviour that could disrupt the operator.
//!
//! Ownership split: this module owns the STORAGE and the meaning of `mode`
//! for AMQ delivery and the Orchestrator watchdog. The watch engine owns what
//! `watch_rule_arm` and `auto_clear_on_task_done` mean at runtime, and reads
//! them through `Engine::session_settings`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::ProviderKind;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSettings {
    /// Context mode. Drives auto-clear policy, the AMQ Worker postscript and
    /// the Orchestrator watchdog. Default: [`ContextMode::Attended`].
    pub mode: ContextMode,

    /// Permission/approval bypass for this session. Claude and Codex receive
    /// wrapper env vars. Default `false`: the operator must opt in.
    pub yolo_permissions: bool,

    /// Per-rule arm/disarm overrides keyed by rule index in the provider's
    /// `[providers.<X>.watch]` array. Absence means the rule's config-default
    /// arm state. Persisted so a manual disarm survives restart.
    pub watch_rule_arm: HashMap<usize, bool>,

    /// Built-in auto-clear-after-task-done rule, only meaningful when
    /// `mode == ContextMode::Worker`. Default `false` even for workers: the
    /// operator opts in twice (Worker mode AND this box).
    pub auto_clear_on_task_done: bool,

    /// Per-session override for `[amq.inject].verify_envelope`. `None`
    /// inherits the global default. Applied at PTY spawn time.
    pub verify_envelope_override: Option<bool>,

    /// Per-session provider-instruction text exported as `DUX_SYSTEM_PROMPT`
    /// at spawn time; the wrappers translate it per provider. Whitespace-only
    /// values are treated as `None`.
    pub system_prompt: Option<String>,
}

/// Built-in role policy for sessions marked [`ContextMode::Orchestrator`].
///
/// Injected at the session-settings layer so the policy follows the role,
/// independent of whether the underlying harness is Claude or Codex.
pub const ORCHESTRATOR_SYSTEM_PROMPT: &str = "\
You are running in Dux Orchestrator mode.

Operating rules:
- Orchestrate only. Do not implement code, edit files, run build/test/lint commands, commit, push, or do hands-on worker tasks unless the user explicitly overrides Orchestrator mode.
- Use `dux peer send` to assign, poll, unblock, and review worker agents. Keep implementation in Worker sessions. Do not call `amq` or Claude Peers directly for normal peer routing; Dux chooses the transport.
- Maintain a visible status model for each active worker: handle, branch/worktree, task, last status, blockers, next checkpoint, and expected proof.
- Proactively poll active workers every 10-15 minutes, and sooner when a worker is blocked, quiet after a promised checkpoint, or affected by a dependency change.
- Make polls specific: ask for current status, blockers, ETA, next command/result, and the proof that will demonstrate completion.
- Do not flood workers. Send at most one nudge per worker per checkpoint unless new information or a missed deadline justifies escalation.
- Demand production-standard completion: clear scope, no unrelated edits, appropriate tests/lint/security checks, reviewable diffs, and concise handoff notes.
- Escalate to the human when workers disagree, are blocked by missing decisions, or repeatedly miss checkpoints.";

/// Operator-declared "what is this session for".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Operator-managed session. Never auto-cleared, never sees the
    /// task-done postscript on AMQ wakes. The default.
    #[default]
    Attended,
    /// Coordinator that talks to peers. Persistent context; receives the
    /// built-in Orchestrator policy and periodic checkpoint nudges.
    Orchestrator,
    /// Stateless processor. AMQ wakes get a postscript asking for the
    /// `[task-done]` sentinel; auto-clear may apply when opted in.
    Worker,
}

impl ContextMode {
    pub const ALL: [ContextMode; 3] = [Self::Attended, Self::Orchestrator, Self::Worker];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Attended => "attended",
            Self::Orchestrator => "orchestrator",
            Self::Worker => "worker",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Attended => "Attended",
            Self::Orchestrator => "Orchestrator",
            Self::Worker => "Worker",
        }
    }
}

/// Env vars derived from [`SessionSettings`] for a spawned agent PTY.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PerSessionEnv {
    pub vars: Vec<(String, String)>,
}

impl SessionSettings {
    /// Parse from the SQLite `session_settings` column. `None`, blank, or
    /// malformed JSON all return `Self::default()` (asymmetric fail-safe).
    pub fn parse_or_default(raw: Option<&str>) -> Self {
        let Some(raw) = raw else {
            return Self::default();
        };
        if raw.trim().is_empty() {
            return Self::default();
        }
        match serde_json::from_str(raw) {
            Ok(s) => s,
            Err(err) => {
                crate::logger::warn(&format!(
                    "session_settings JSON malformed; falling back to default: {err}"
                ));
                Self::default()
            }
        }
    }

    /// Serialise for storage. Always valid JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("SessionSettings serialises")
    }

    /// Whether every knob is at its default, so storage can keep NULL.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The system prompt the agent should run with: the Orchestrator policy
    /// (plus any custom text) in Orchestrator mode, otherwise the non-blank
    /// custom text, otherwise nothing.
    pub fn effective_system_prompt(&self) -> Option<String> {
        let custom = self
            .system_prompt
            .as_deref()
            .filter(|s| !s.trim().is_empty());
        match (self.mode, custom) {
            (ContextMode::Orchestrator, Some(custom)) => {
                Some(format!("{ORCHESTRATOR_SYSTEM_PROMPT}\n\n{custom}"))
            }
            (ContextMode::Orchestrator, None) => Some(ORCHESTRATOR_SYSTEM_PROMPT.to_string()),
            (_, Some(custom)) => Some(custom.to_string()),
            (_, None) => None,
        }
    }

    /// Translate settings into env vars for the spawned PTY child. The AMQ
    /// wrappers (`claude-amq`, `codex-amq`, ...) read these to decide CLI
    /// flags. `verify_envelope_global` is `[amq.inject].verify_envelope`,
    /// used when `verify_envelope_override` is `None`.
    ///
    /// Every agent launch appends these through
    /// [`crate::agent_env::session_settings_env`], after the DUX_* identity
    /// and before the user's `[env]`.
    pub fn to_pty_env(
        &self,
        provider: &ProviderKind,
        verify_envelope_global: bool,
    ) -> PerSessionEnv {
        let mut vars: Vec<(String, String)> = Vec::new();
        if self.yolo_permissions {
            match provider.as_str() {
                "claude" => vars.push(("CLAUDE_AMQ_YOLO".into(), "1".into())),
                "codex" => vars.push(("CODEX_AMQ_YOLO".into(), "1".into())),
                // OpenCode gets `--auto` through its launch args; other
                // providers have no YOLO mapping and the knob is a no-op.
                _ => {}
            }
        }
        // Always export DUX_AMQ_VERIFY so the bridge sees a deterministic
        // value. Per-session override beats global.
        let strict = self
            .verify_envelope_override
            .unwrap_or(verify_envelope_global);
        vars.push((
            "DUX_AMQ_VERIFY".into(),
            if strict { "1" } else { "0" }.into(),
        ));
        // Whitespace-only prompts never reach the wrapper: claude's
        // `--append-system-prompt " "` would still alter the model prompt.
        if let Some(prompt) = self.effective_system_prompt()
            && !prompt.trim().is_empty()
        {
            vars.push(("DUX_SYSTEM_PROMPT".into(), prompt));
        }
        PerSessionEnv { vars }
    }

    /// Extra CLI args YOLO adds for providers that take it as a flag rather
    /// than through a wrapper env var (fork c2c44378: OpenCode `--auto`).
    /// Appended after the provider's own args, resume args included.
    ///
    /// INTEGRATION: the agent launch argv builder (resume worker, palmtree,
    /// with the peer worker's launch env) must append these for a session
    /// whose settings have `yolo_permissions`.
    pub fn yolo_launch_args(&self, provider: &ProviderKind) -> Vec<String> {
        if !self.yolo_permissions {
            return Vec::new();
        }
        match provider.as_str() {
            "opencode" => vec!["--auto".to_string()],
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_value<'a>(env: &'a PerSessionEnv, key: &str) -> Option<&'a str> {
        env.vars
            .iter()
            .find_map(|(k, v)| (k == key).then_some(v.as_str()))
    }

    #[test]
    fn default_is_attended_and_inert() {
        let s = SessionSettings::default();
        assert_eq!(s.mode, ContextMode::Attended);
        assert!(!s.yolo_permissions);
        assert!(!s.auto_clear_on_task_done);
        assert!(s.is_default());
        assert_eq!(s.effective_system_prompt(), None);
        assert!(
            s.yolo_launch_args(&ProviderKind::from_str("opencode"))
                .is_empty()
        );
    }

    /// c2c44378: OpenCode's YOLO is `--auto` on the command line, after any
    /// resume args; no other provider gets a flag from it.
    #[test]
    fn opencode_yolo_launch_adds_auto() {
        let opencode = ProviderKind::from_str("opencode");
        let yolo = SessionSettings {
            yolo_permissions: true,
            ..SessionSettings::default()
        };
        assert!(
            SessionSettings::default()
                .yolo_launch_args(&opencode)
                .is_empty()
        );
        assert_eq!(yolo.yolo_launch_args(&opencode), vec!["--auto"]);
        let mut resumed = vec!["--continue".to_string()];
        resumed.extend(yolo.yolo_launch_args(&opencode));
        assert_eq!(resumed, vec!["--continue", "--auto"]);
        for other in ["claude", "codex", "jcode"] {
            assert!(
                yolo.yolo_launch_args(&ProviderKind::from_str(other))
                    .is_empty()
            );
        }
    }

    #[test]
    fn parse_or_default_is_fail_safe() {
        assert_eq!(
            SessionSettings::parse_or_default(None),
            SessionSettings::default()
        );
        assert_eq!(
            SessionSettings::parse_or_default(Some("  ")),
            SessionSettings::default()
        );
        assert_eq!(
            SessionSettings::parse_or_default(Some("{not json")),
            SessionSettings::default()
        );
        // Unknown fields and missing fields are both tolerated.
        let s = SessionSettings::parse_or_default(Some(r#"{"mode":"worker","future":1}"#));
        assert_eq!(s.mode, ContextMode::Worker);
        assert!(!s.yolo_permissions);
    }

    #[test]
    fn json_round_trips_every_field() {
        let s = SessionSettings {
            mode: ContextMode::Orchestrator,
            yolo_permissions: true,
            watch_rule_arm: HashMap::from([(2, false)]),
            auto_clear_on_task_done: true,
            verify_envelope_override: Some(true),
            system_prompt: Some("line one\nline two".into()),
        };
        assert_eq!(SessionSettings::parse_or_default(Some(&s.to_json())), s);
    }

    #[test]
    fn to_pty_env_maps_yolo_per_provider() {
        let s = SessionSettings {
            yolo_permissions: true,
            ..SessionSettings::default()
        };
        let claude = s.to_pty_env(&ProviderKind::new("claude"), false);
        assert_eq!(env_value(&claude, "CLAUDE_AMQ_YOLO"), Some("1"));
        let codex = s.to_pty_env(&ProviderKind::new("codex"), false);
        assert_eq!(env_value(&codex, "CODEX_AMQ_YOLO"), Some("1"));
        let gemini = s.to_pty_env(&ProviderKind::new("gemini"), false);
        assert!(!gemini.vars.iter().any(|(k, _)| k.ends_with("_AMQ_YOLO")));
        let off = SessionSettings::default().to_pty_env(&ProviderKind::new("claude"), false);
        assert_eq!(env_value(&off, "CLAUDE_AMQ_YOLO"), None);
    }

    /// Fork `spawn_with_env_falls_back_to_global_verify_envelope`.
    #[test]
    fn verify_envelope_falls_back_to_global_and_override_wins() {
        let provider = ProviderKind::new("claude");
        let s = SessionSettings::default();
        assert_eq!(
            env_value(&s.to_pty_env(&provider, true), "DUX_AMQ_VERIFY"),
            Some("1")
        );
        assert_eq!(
            env_value(&s.to_pty_env(&provider, false), "DUX_AMQ_VERIFY"),
            Some("0")
        );
        let off = SessionSettings {
            verify_envelope_override: Some(false),
            ..SessionSettings::default()
        };
        assert_eq!(
            env_value(&off.to_pty_env(&provider, true), "DUX_AMQ_VERIFY"),
            Some("0")
        );
        let on = SessionSettings {
            verify_envelope_override: Some(true),
            ..SessionSettings::default()
        };
        assert_eq!(
            env_value(&on.to_pty_env(&provider, false), "DUX_AMQ_VERIFY"),
            Some("1")
        );
    }

    /// Fork `to_pty_env_emits_system_prompt_only_when_set_and_non_blank`.
    #[test]
    fn system_prompt_exported_only_when_non_blank() {
        let provider = ProviderKind::new("claude");
        for blank in [None, Some(String::new()), Some("   \n\t  ".to_string())] {
            let env = SessionSettings {
                system_prompt: blank.clone(),
                ..SessionSettings::default()
            }
            .to_pty_env(&provider, false);
            assert_eq!(env_value(&env, "DUX_SYSTEM_PROMPT"), None, "{blank:?}");
        }
        let env = SessionSettings {
            system_prompt: Some("line one\nline two".into()),
            ..SessionSettings::default()
        }
        .to_pty_env(&provider, false);
        assert_eq!(
            env_value(&env, "DUX_SYSTEM_PROMPT"),
            Some("line one\nline two")
        );
    }

    /// Fork `to_pty_env_emits_system_prompt_for_every_provider`: dux never
    /// pre-filters by provider; the wrapper decides.
    #[test]
    fn system_prompt_exported_for_every_provider() {
        let s = SessionSettings {
            system_prompt: Some("custom".into()),
            ..SessionSettings::default()
        };
        for name in ["claude", "codex", "gemini", "opencode", "jcode"] {
            let env = s.to_pty_env(&ProviderKind::new(name), false);
            assert_eq!(
                env_value(&env, "DUX_SYSTEM_PROMPT"),
                Some("custom"),
                "{name}"
            );
        }
    }

    /// Fork `to_pty_env_adds_builtin_orchestrator_policy`.
    #[test]
    fn orchestrator_mode_exports_builtin_policy_plus_custom_text() {
        let provider = ProviderKind::new("codex");
        let s = SessionSettings {
            mode: ContextMode::Orchestrator,
            ..SessionSettings::default()
        };
        let env = s.to_pty_env(&provider, false);
        let prompt = env_value(&env, "DUX_SYSTEM_PROMPT").expect("policy");
        assert!(prompt.contains("Dux Orchestrator mode"));
        assert!(prompt.contains("Orchestrate only"));
        assert!(prompt.contains("dux peer send"));

        let custom = SessionSettings {
            mode: ContextMode::Orchestrator,
            system_prompt: Some("Project-specific orchestration rule.".into()),
            ..SessionSettings::default()
        };
        let env = custom.to_pty_env(&provider, false);
        let prompt = env_value(&env, "DUX_SYSTEM_PROMPT").expect("policy");
        assert!(prompt.starts_with(ORCHESTRATOR_SYSTEM_PROMPT));
        assert!(prompt.ends_with("Project-specific orchestration rule."));
    }

    #[test]
    fn context_mode_serialises_lowercase() {
        for mode in ContextMode::ALL {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
        }
    }
}
