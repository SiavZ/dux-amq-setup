//! Orchestrator watchdog prompts and peer selection (pure). The engine-side
//! timer lives in `crate::engine::amq`.

use std::fmt::Write as _;

use crate::session_settings::ContextMode;

/// Cap on peers listed in one checkpoint prompt, so a very large workspace
/// cannot produce an unbounded paste.
pub const MAX_CHECKPOINT_PEERS: usize = 40;

/// One live worker as listed in a checkpoint prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrchestratorPeer {
    pub handle: String,
    pub label: String,
    pub provider: String,
    pub mode: ContextMode,
    pub branch: String,
    pub worktree: String,
}

/// The launch-time policy prompt typed into providers with no system-prompt
/// flag. It never asks for a poll: polling on startup flooded workers every
/// restart (931f5ae0).
pub fn build_orchestrator_startup_policy_prompt(
    policy: &str,
    peers: &[OrchestratorPeer],
) -> String {
    let mut out = String::from("[Dux Orchestrator startup policy]\n\n");
    out.push_str(policy.trim());
    if peers.is_empty() {
        out.push_str(
            "\n\nAcknowledge Orchestrator mode briefly. There are no active worker agents yet; wait for assignments or Dux peer messages, and do not do hands-on implementation work.",
        );
    } else {
        let _ = writeln!(
            out,
            "\n\nAcknowledge Orchestrator mode briefly. Dux sees {} active worker agent(s), but do not poll them on startup. Wait for assignments, Dux peer messages, or the next scheduled Dux checkpoint.",
            peers.len()
        );
    }
    out
}

/// The built-in periodic checkpoint prompt listing live workers.
pub fn build_orchestrator_checkpoint_prompt(peers: &[OrchestratorPeer]) -> String {
    let mut out = String::from(
        "[Dux Orchestrator checkpoint]\n\n\
Poll active worker agents now through `dux peer send`. Do not do their implementation work yourself.\n\n\
Active agents:\n",
    );
    for peer in peers.iter().take(MAX_CHECKPOINT_PEERS) {
        let _ = writeln!(
            out,
            "- {handle} ({provider}, {mode}): {label}; branch={branch}; worktree={worktree}",
            handle = peer.handle,
            provider = peer.provider,
            mode = peer.mode.as_str(),
            label = peer.label,
            branch = peer.branch,
            worktree = peer.worktree,
        );
    }
    if peers.len() > MAX_CHECKPOINT_PEERS {
        let _ = writeln!(
            out,
            "- ... {} more live agents omitted from this checkpoint prompt",
            peers.len() - MAX_CHECKPOINT_PEERS
        );
    }
    out.push_str(
        "\nFor each active agent, use `dux peer send <handle> \"...\"` to ask for status, blockers, ETA, next command/result, and the proof that will demonstrate completion. Do not call `amq` or Claude Peers directly for normal peer routing; Dux chooses the transport.\n\
Push stale or blocked agents with a concrete next checkpoint. Demand scoped diffs, relevant tests/lint/security checks, and concise handoff notes. Escalate to the human when agents disagree, need a decision, or repeatedly miss checkpoints.",
    );
    out
}

/// `[amq.orchestrator].checkpoint_prompt` verbatim when non-blank, else the
/// built-in template (3707af8c).
pub fn resolve_orchestrator_checkpoint_prompt(custom: &str, peers: &[OrchestratorPeer]) -> String {
    let trimmed = custom.trim();
    if trimmed.is_empty() {
        build_orchestrator_checkpoint_prompt(peers)
    } else {
        trimmed.to_string()
    }
}

/// Claude receives the policy through `--append-system-prompt` at launch;
/// every other provider needs it typed into the PTY once (c2622dc0).
pub fn provider_needs_pty_orchestrator_policy(provider: &str) -> bool {
    !provider.eq_ignore_ascii_case("claude")
}

/// A checkpoint lists only LIVE WORKERS in the SAME PROJECT, never the
/// orchestrator itself (cc7005d0: cross-project polling flooded workers).
pub fn is_orchestrator_checkpoint_peer(
    candidate_id: &str,
    candidate_project_id: Option<&str>,
    candidate_mode: ContextMode,
    candidate_is_live: bool,
    orchestrator_id: &str,
    orchestrator_project_id: Option<&str>,
) -> bool {
    candidate_is_live
        && candidate_id != orchestrator_id
        && candidate_project_id == orchestrator_project_id
        && matches!(candidate_mode, ContextMode::Worker)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_settings::ORCHESTRATOR_SYSTEM_PROMPT;

    fn qa() -> OrchestratorPeer {
        OrchestratorPeer {
            handle: "front-end-qa".to_string(),
            label: "QA".to_string(),
            provider: "codex".to_string(),
            mode: ContextMode::Worker,
            branch: "feature/qa".to_string(),
            worktree: "/tmp/Front-end-QA".to_string(),
        }
    }

    #[test]
    fn checkpoint_prompt_lists_handles_and_orchestration_rules() {
        let prompt = build_orchestrator_checkpoint_prompt(&[qa()]);
        assert!(prompt.contains("front-end-qa"));
        assert!(prompt.contains("Do not do their implementation work yourself"));
        // Pins the command the orchestrator is told to run; the `dux peer
        // send` CLI itself is tested in `peer::router`.
        assert!(prompt.contains("dux peer send <handle>"));
        assert!(prompt.contains("status, blockers, ETA"));
    }

    #[test]
    fn checkpoint_prompt_caps_the_peer_list() {
        let peers: Vec<_> = (0..MAX_CHECKPOINT_PEERS + 3)
            .map(|i| OrchestratorPeer {
                handle: format!("w{i}"),
                ..qa()
            })
            .collect();
        let prompt = build_orchestrator_checkpoint_prompt(&peers);
        assert!(prompt.contains("- ... 3 more live agents omitted"));
        assert!(!prompt.contains(&format!("- w{} ", MAX_CHECKPOINT_PEERS)));
    }

    #[test]
    fn custom_checkpoint_prompt_replaces_builtin_template() {
        let prompt = resolve_orchestrator_checkpoint_prompt(
            "Checkpoint from the operator: keep the goal moving.\n",
            &[qa()],
        );
        assert_eq!(
            prompt,
            "Checkpoint from the operator: keep the goal moving."
        );
    }

    #[test]
    fn blank_custom_checkpoint_prompt_falls_back_to_builtin() {
        for blank in ["", "   ", "\n\t "] {
            let prompt = resolve_orchestrator_checkpoint_prompt(blank, &[qa()]);
            assert!(prompt.contains("Dux Orchestrator checkpoint"));
            assert!(prompt.contains("front-end-qa"));
        }
    }

    #[test]
    fn startup_policy_prompt_carries_policy_without_peers() {
        let prompt = build_orchestrator_startup_policy_prompt(ORCHESTRATOR_SYSTEM_PROMPT, &[]);
        assert!(prompt.contains("Dux Orchestrator startup policy"));
        assert!(prompt.contains("Orchestrate only"));
        assert!(prompt.contains("There are no active worker agents yet"));
    }

    #[test]
    fn startup_policy_prompt_does_not_trigger_worker_poll() {
        let prompt = build_orchestrator_startup_policy_prompt(ORCHESTRATOR_SYSTEM_PROMPT, &[qa()]);
        assert!(prompt.contains("do not poll them on startup"));
        assert!(!prompt.contains("dux peer send <handle>"));
        assert!(!prompt.contains("Dux Orchestrator checkpoint"));
    }

    #[test]
    fn checkpoint_peers_are_live_workers_in_same_project_only() {
        let a = Some("project-a");
        let b = Some("project-b");
        assert!(is_orchestrator_checkpoint_peer(
            "w1",
            a,
            ContextMode::Worker,
            true,
            "o1",
            a
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "w2",
            b,
            ContextMode::Worker,
            true,
            "o1",
            a
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "a1",
            a,
            ContextMode::Attended,
            true,
            "o1",
            a
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "w3",
            a,
            ContextMode::Worker,
            false,
            "o1",
            a
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "o1",
            a,
            ContextMode::Worker,
            true,
            "o1",
            a
        ));
    }

    #[test]
    fn codex_needs_pty_policy_but_claude_does_not() {
        assert!(provider_needs_pty_orchestrator_policy("codex"));
        assert!(provider_needs_pty_orchestrator_policy("gemini"));
        assert!(provider_needs_pty_orchestrator_policy("jcode"));
        assert!(!provider_needs_pty_orchestrator_policy("claude"));
        assert!(!provider_needs_pty_orchestrator_policy("Claude"));
    }
}
