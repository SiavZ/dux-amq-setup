//! Pure delivery policy for AMQ wakes: receiver matching, provider byte
//! encoding, hold rules and the Worker-mode postscript. No I/O, no engine
//! state, so every rule is unit-testable. The engine-side state machine that
//! applies these lives in `crate::engine::amq`.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::session_settings::ContextMode;

/// Windows at or beyond this bound mean "always deliver, never hold": the
/// documented `u64::MAX` escape hatch, generalized to any implausibly long
/// value (audit03 P1-19).
pub const ALWAYS_DELIVER_QUIET_WINDOW: Duration = Duration::from_secs(10 * 365 * 24 * 60 * 60);

// The PTY byte encoding is shared with the watch engine: one implementation,
// so a wake and a watch-rule nudge reach a provider byte-identically.
pub use crate::watch::builtin::TASK_DONE_SENTINEL;
pub use crate::watch::delivery::{
    MIN_ENTER_PHASE_DELAY_MS, effective_enter_phase_delay, inject_body_bytes_for_provider,
    submit_key_bytes_for_provider,
};

/// Sanitise a name the same way the AMQ wrappers do: lowercase ASCII, keep
/// `[a-z0-9_-]`, replace anything else with `-`, trim leading/trailing `-`.
///
/// INTEGRATION: the peer worker (seedling) ports the same function as
/// `crate::peer::handle::amq_handle`; delegate to it at merge.
pub fn sanitise_handle(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_ascii_uppercase() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// One session as the receiver matcher sees it.
#[derive(Clone, Copy, Debug)]
pub struct ReceiverCandidate<'a> {
    pub id: &'a str,
    pub handle: &'a str,
    pub branch: &'a str,
    pub directory: &'a str,
}

/// Resolve an AMQ receiver. The immutable agent handle is authoritative;
/// the session id is an operator escape hatch; the worktree basename and
/// branch are legacy aliases accepted only when they identify exactly one
/// session (shared workspaces put several agents in one directory).
pub fn match_receiver<'a>(sessions: &[ReceiverCandidate<'a>], receiver: &str) -> Option<&'a str> {
    if let Some(s) = sessions.iter().find(|s| s.handle == receiver) {
        return Some(s.id);
    }
    if let Some(s) = sessions.iter().find(|s| s.id == receiver) {
        return Some(s.id);
    }
    exactly_one(sessions.iter().filter_map(|s| {
        std::path::Path::new(s.directory)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| sanitise_handle(name) == receiver)
            .map(|_| s.id)
    }))
    .or_else(|| {
        exactly_one(
            sessions
                .iter()
                .filter(|s| !s.branch.is_empty() && sanitise_handle(s.branch) == receiver)
                .map(|s| s.id),
        )
    })
}

fn exactly_one<'a>(mut matches: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let first = matches.next()?;
    matches.next().is_none().then_some(first)
}

/// Apply the Worker-mode postscript. Worker sessions get an authoritative
/// role update (so it overrides an earlier Orchestrator instruction, c69a09f0)
/// and a request to end with the task-done sentinel. Other modes pass through.
pub fn apply_inject_postscript(body: &str, mode: ContextMode) -> String {
    match mode {
        ContextMode::Worker => format!(
            "{body}\n\n[Dux Worker mode] The operator currently designates this session as a Worker. This supersedes any earlier Dux Orchestrator-mode instruction in this conversation. Execute the assigned task directly instead of orchestrating or delegating it. When this task is complete, end your reply with the literal token {TASK_DONE_SENTINEL} so the orchestration layer knows to clean up.",
        ),
        ContextMode::Attended | ContextMode::Orchestrator => body.to_string(),
    }
}

/// Whether a wake aimed at a session the operator is focused on must wait.
///
/// - `quiet == 0`: legacy "always hold while focused".
/// - `quiet >= ALWAYS_DELIVER_QUIET_WINDOW`: always deliver.
/// - otherwise hold iff the last user keystroke is within the window; with
///   no recorded keystroke the operator is idle and delivery flows.
pub fn should_hold_for_quiet_window(
    last_keystroke: Option<Instant>,
    now: Instant,
    quiet: Duration,
) -> bool {
    if quiet.is_zero() {
        return true;
    }
    if quiet >= ALWAYS_DELIVER_QUIET_WINDOW {
        return false;
    }
    last_keystroke.is_some_and(|t| now.duration_since(t) < quiet)
}

/// Where a queued wake is in two-phase delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryPhase {
    /// No bytes have reached the provider prompt; preflight holds apply.
    TypeBody,
    /// The body is already in the prompt. Only Enter may follow; preflight
    /// holds must not run again and the body must never be retyped.
    SubmitPending { typed_at: Option<Instant> },
}

/// Post-delivery cooldown is backpressure between separate wakes. It must
/// never delay phase 2 of the wake already typed, or the prompt sits
/// unsubmitted until the cooldown ends.
pub fn should_hold_for_post_delivery_cooldown(
    phase: Option<DeliveryPhase>,
    cooldown_until: Option<Instant>,
    now: Instant,
) -> bool {
    matches!(phase, Some(DeliveryPhase::TypeBody))
        && cooldown_until.is_some_and(|until| now < until)
}

/// Soft timeout warning: the first pending instant is established on the
/// first deferral (busy, no PTY, no session alike, audit03 P1-18), and the
/// warning fires once per receiver.
pub fn timeout_warning_due(
    first_pending_at: &mut HashMap<String, Instant>,
    warned: &mut HashSet<String>,
    receiver: &str,
    now: Instant,
    timeout: Duration,
) -> bool {
    let first = *first_pending_at.entry(receiver.to_string()).or_insert(now);
    now.duration_since(first) >= timeout && warned.insert(receiver.to_string())
}

/// Why the drainer held a message, for the rate-limited debug log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldReason<'a> {
    NoSession,
    UserTyping,
    PtyGone,
    BusyMarker(&'a str),
    PostDeliveryCooldown,
    StartupGrace,
}

impl HoldReason<'_> {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoSession => "no_matching_session",
            Self::UserTyping => "user_typing_in_target_session",
            Self::PtyGone => "pty_gone",
            Self::BusyMarker(_) => "busy_marker_detected",
            Self::PostDeliveryCooldown => "post_delivery_cooldown",
            Self::StartupGrace => "startup_grace",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderKind;

    fn c<'a>(id: &'a str, handle: &'a str, branch: &'a str, dir: &'a str) -> ReceiverCandidate<'a> {
        ReceiverCandidate {
            id,
            handle,
            branch,
            directory: dir,
        }
    }

    #[test]
    fn fresh_pending_receiver_warns_once_after_timeout() {
        let start = Instant::now();
        let mut first = HashMap::new();
        let mut warned = HashSet::new();
        let t = Duration::from_secs(30);
        assert!(!timeout_warning_due(&mut first, &mut warned, "r", start, t));
        assert!(timeout_warning_due(
            &mut first,
            &mut warned,
            "r",
            start + t,
            t
        ));
        assert!(!timeout_warning_due(
            &mut first,
            &mut warned,
            "r",
            start + t * 2,
            t
        ));
    }

    #[test]
    fn codex_body_uses_bracketed_paste() {
        let p = ProviderKind::from_str("codex");
        assert_eq!(
            inject_body_bytes_for_provider("a\nb", Some(&p)),
            b"\x1b[200~a\nb\x1b[201~"
        );
    }

    #[test]
    fn claude_long_body_uses_bracketed_paste() {
        let p = ProviderKind::from_str("claude");
        let body = "x".repeat(1537);
        let payload = inject_body_bytes_for_provider(&body, Some(&p));
        assert_eq!(&payload[..6], b"\x1b[200~");
        assert_eq!(&payload[6..payload.len() - 6], body.as_bytes());
        assert_eq!(&payload[payload.len() - 6..], b"\x1b[201~");
    }

    #[test]
    fn codex_body_normalizes_crlf_inside_bracketed_paste() {
        let p = ProviderKind::from_str("codex");
        assert_eq!(
            inject_body_bytes_for_provider("a\r\nb\rc", Some(&p)),
            b"\x1b[200~a\nb\nc\x1b[201~"
        );
    }

    #[test]
    fn codex_body_escapes_control_chars_inside_bracketed_paste() {
        let p = ProviderKind::from_str("codex");
        assert_eq!(
            inject_body_bytes_for_provider("a\x1b[201~b", Some(&p)),
            b"\x1b[200~a\\x1b[201~b\x1b[201~"
        );
    }

    #[test]
    fn non_bracketed_paste_body_uses_macro_payload_newline_encoding() {
        for name in ["gemini", "custom", "jcode"] {
            let p = ProviderKind::from_str(name);
            assert_eq!(
                inject_body_bytes_for_provider("a\nb", Some(&p)),
                b"a\x1b\rb"
            );
        }
        assert_eq!(inject_body_bytes_for_provider("a\nb", None), b"a\x1b\rb");
    }

    #[test]
    fn every_provider_submits_with_raw_enter() {
        for name in ["claude", "codex", "gemini", "custom"] {
            let p = ProviderKind::from_str(name);
            assert_eq!(submit_key_bytes_for_provider(Some(&p)), b"\r");
        }
        assert_eq!(submit_key_bytes_for_provider(None), b"\r");
    }

    #[test]
    fn sanitise_matches_the_wrappers() {
        assert_eq!(sanitise_handle("ALICE"), "alice");
        assert_eq!(sanitise_handle("Feature/Login.v2"), "feature-login-v2");
        assert_eq!(sanitise_handle("foo bar"), "foo-bar");
        assert_eq!(sanitise_handle("--foo--"), "foo");
        assert_eq!(sanitise_handle("///foo///"), "foo");
        assert_eq!(sanitise_handle("watch-rules-phase3"), "watch-rules-phase3");
        assert_eq!(sanitise_handle("a1b2_c3"), "a1b2_c3");
        assert_eq!(sanitise_handle("..."), "");
        assert_eq!(sanitise_handle("///"), "");
    }

    /// The production bug: branch renamed, wrapper derived the handle from the
    /// worktree basename; a branch-only matcher orphaned every wake.
    #[test]
    fn match_receiver_matches_worktree_basename_when_branch_diverges() {
        let s = [c(
            "session-uuid-1",
            "worker-1",
            "fix/qa-s45-charge-schema-paymentmethod",
            "/data/state/dux/worktrees/Jobzy-Front-end/Front-end-QA",
        )];
        assert_eq!(match_receiver(&s, "front-end-qa"), Some("session-uuid-1"));
    }

    #[test]
    fn match_receiver_falls_back_to_branch_name() {
        let s = [c("id2", "worker-2", "feature-login", "/some/path/legacy")];
        assert_eq!(match_receiver(&s, "feature-login"), Some("id2"));
    }

    #[test]
    fn match_receiver_accepts_session_id_for_operator_addressing() {
        let s = [c("af882c2d", "worker-3", "fix/foo", "/wt/Bar")];
        assert_eq!(match_receiver(&s, "af882c2d"), Some("af882c2d"));
    }

    #[test]
    fn match_receiver_returns_none_when_nothing_matches() {
        let s = [
            c("id1", "worker-1", "main", "/wt/main"),
            c("id2", "worker-2", "dev", "/wt/dev"),
        ];
        assert_eq!(match_receiver(&s, "front-end-qa"), None);
        assert_eq!(match_receiver(&[], "anything"), None);
    }

    #[test]
    fn match_receiver_basename_beats_branch() {
        let s = [
            c("idA", "worker-a", "fix/random", "/wt/Alice"),
            c("idB", "worker-b", "alice", "/wt/something-else"),
        ];
        assert_eq!(match_receiver(&s, "alice"), Some("idA"));
    }

    #[test]
    fn handle_beats_every_alias() {
        let s = [
            c("idA", "alice", "x", "/wt/x"),
            c("idB", "worker-b", "alice", "/wt/Alice"),
        ];
        assert_eq!(match_receiver(&s, "alice"), Some("idA"));
    }

    #[test]
    fn shared_workspace_routes_by_handle_and_rejects_ambiguous_legacy_aliases() {
        let s = [
            c("idA", "frontend-a", "development", "/repo/Jobzy-Front-end"),
            c("idB", "frontend-b", "development", "/repo/Jobzy-Front-end"),
        ];
        assert_eq!(match_receiver(&s, "frontend-b"), Some("idB"));
        assert_eq!(match_receiver(&s, "jobzy-front-end"), None);
        assert_eq!(match_receiver(&s, "development"), None);
    }

    #[test]
    fn postscript_appended_for_worker_mode_and_overrides_orchestrator_role() {
        let body = "Please review the design doc.";
        let out = apply_inject_postscript(body, ContextMode::Worker);
        assert!(out.starts_with(body));
        assert!(out.contains(TASK_DONE_SENTINEL));
        assert!(out.contains("[Dux Worker mode]"));
        assert!(out.contains("supersedes any earlier Dux Orchestrator-mode instruction"));
    }

    #[test]
    fn postscript_skipped_for_attended_and_orchestrator() {
        let body = "ad-hoc question";
        assert_eq!(apply_inject_postscript(body, ContextMode::Attended), body);
        assert_eq!(
            apply_inject_postscript(body, ContextMode::Orchestrator),
            body
        );
    }

    #[test]
    fn quiet_window_rules() {
        let now = Instant::now();
        let q = Duration::from_secs(60);
        assert!(should_hold_for_quiet_window(
            Some(now - Duration::from_secs(5)),
            now,
            q
        ));
        assert!(!should_hold_for_quiet_window(
            Some(now - Duration::from_secs(120)),
            now,
            q
        ));
        assert!(!should_hold_for_quiet_window(None, now, q));
        // Boundary: exactly at the edge counts as no longer typing.
        assert!(!should_hold_for_quiet_window(Some(now - q), now, q));
    }

    #[test]
    fn quiet_window_zero_always_holds() {
        let now = Instant::now();
        assert!(should_hold_for_quiet_window(None, now, Duration::ZERO));
        assert!(should_hold_for_quiet_window(
            Some(now - Duration::from_secs(10_000)),
            now,
            Duration::ZERO
        ));
    }

    #[test]
    fn quiet_window_huge_values_always_deliver() {
        let now = Instant::now();
        for q in [
            Duration::from_secs(u64::MAX),
            Duration::from_secs(u64::MAX / 2),
            Duration::from_secs(100 * 365 * 24 * 60 * 60),
        ] {
            assert!(!should_hold_for_quiet_window(Some(now), now, q));
        }
        let below = ALWAYS_DELIVER_QUIET_WINDOW - Duration::from_secs(1);
        assert!(should_hold_for_quiet_window(Some(now), now, below));
    }

    #[test]
    fn phase_delay_nonzero_values_are_floored() {
        assert_eq!(effective_enter_phase_delay(1), Duration::from_millis(250));
        assert_eq!(effective_enter_phase_delay(50), Duration::from_millis(250));
        assert_eq!(effective_enter_phase_delay(500), Duration::from_millis(500));
        assert_eq!(effective_enter_phase_delay(0), Duration::ZERO);
    }

    #[test]
    fn post_delivery_cooldown_holds_only_before_body_is_typed() {
        let now = Instant::now();
        let future = now + Duration::from_secs(10);
        assert!(should_hold_for_post_delivery_cooldown(
            Some(DeliveryPhase::TypeBody),
            Some(future),
            now
        ));
        assert!(!should_hold_for_post_delivery_cooldown(
            Some(DeliveryPhase::SubmitPending {
                typed_at: Some(now)
            }),
            Some(future),
            now
        ));
        assert!(!should_hold_for_post_delivery_cooldown(
            Some(DeliveryPhase::TypeBody),
            Some(now - Duration::from_secs(1)),
            now
        ));
        assert!(!should_hold_for_post_delivery_cooldown(
            None,
            Some(now),
            now
        ));
        assert!(!should_hold_for_post_delivery_cooldown(
            Some(DeliveryPhase::TypeBody),
            None,
            now
        ));
    }
}
