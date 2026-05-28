use super::*;

use std::fmt::Write as _;

const MAX_CHECKPOINT_PEERS: usize = 40;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OrchestratorPeer {
    pub(crate) handle: String,
    pub(crate) label: String,
    pub(crate) provider: String,
    pub(crate) mode: ContextMode,
    pub(crate) branch: String,
    pub(crate) worktree: String,
}

struct OrchestratorTarget {
    receiver: String,
    provider: String,
    project_id: String,
    startup_policy: String,
}

pub(crate) fn build_orchestrator_startup_policy_prompt(
    policy: &str,
    peers: &[OrchestratorPeer],
) -> String {
    let mut out = String::from("[Dux Orchestrator startup policy]\n\n");
    out.push_str(policy.trim());
    if peers.is_empty() {
        out.push_str(
            "\n\nAcknowledge Orchestrator mode briefly. There are no active worker agents yet; wait for assignments or AMQ messages, and do not do hands-on implementation work.",
        );
    } else {
        let _ = writeln!(
            out,
            "\n\nAcknowledge Orchestrator mode briefly. Dux sees {} active worker agent(s), but do not poll them on startup. Wait for assignments, AMQ messages, or the next scheduled Dux checkpoint.",
            peers.len()
        );
    }
    out
}

pub(crate) fn build_orchestrator_checkpoint_prompt(peers: &[OrchestratorPeer]) -> String {
    let mut out = String::from(
        "[Dux Orchestrator checkpoint]\n\n\
Poll active worker agents now through AMQ. Do not do their implementation work yourself.\n\n\
Active agents:\n",
    );
    for peer in peers.iter().take(MAX_CHECKPOINT_PEERS) {
        let _ = writeln!(
            out,
            "- {handle} ({provider}, {mode}): {label}; branch={branch}; worktree={worktree}",
            handle = peer.handle,
            provider = peer.provider,
            mode = context_mode_label(peer.mode),
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
        "\nFor each active agent, use AMQ to ask for status, blockers, ETA, next command/result, and the proof that will demonstrate completion. Use commands like `amq send --to <handle> --body \"...\"` from inside this session.\n\
Push stale or blocked agents with a concrete next checkpoint. Demand scoped diffs, relevant tests/lint/security checks, and concise handoff notes. Escalate to the human when agents disagree, need a decision, or repeatedly miss checkpoints.",
    );
    out
}

fn context_mode_label(mode: ContextMode) -> &'static str {
    match mode {
        ContextMode::Attended => "attended",
        ContextMode::Orchestrator => "orchestrator",
        ContextMode::Worker => "worker",
    }
}

fn provider_needs_pty_orchestrator_policy(provider: &str) -> bool {
    !provider.eq_ignore_ascii_case("claude")
}

fn is_orchestrator_checkpoint_peer(
    candidate_id: &str,
    candidate_project_id: &str,
    candidate_mode: ContextMode,
    candidate_is_live: bool,
    orchestrator_id: &str,
    orchestrator_project_id: &str,
) -> bool {
    candidate_is_live
        && candidate_id != orchestrator_id
        && candidate_project_id == orchestrator_project_id
        && matches!(candidate_mode, ContextMode::Worker)
}

impl App {
    pub(crate) fn tick_orchestrator_watchdog(&mut self) {
        if !self.config.amq.orchestrator.enabled {
            return;
        }
        let poll_interval_secs = self.config.amq.orchestrator.poll_interval_secs;
        if poll_interval_secs == 0 {
            return;
        }
        let now = Instant::now();
        let poll_interval = Duration::from_secs(poll_interval_secs);
        let orchestrator_ids: Vec<String> = self
            .git
            .sessions
            .iter()
            .filter(|s| s.state.is_live() && matches!(s.settings.mode, ContextMode::Orchestrator))
            .map(|s| s.id.clone())
            .collect();
        if orchestrator_ids.is_empty() {
            return;
        }
        let live_orchestrators: HashSet<String> = orchestrator_ids.iter().cloned().collect();
        self.runtime
            .orchestrator_last_nudged
            .retain(|session_id, _| live_orchestrators.contains(session_id));
        self.runtime
            .orchestrator_policy_injected
            .retain(|session_id| live_orchestrators.contains(session_id));
        let live_orchestrator_projects: HashSet<String> = self
            .git
            .sessions
            .iter()
            .filter(|s| live_orchestrators.contains(&s.id))
            .map(|s| s.project_id.clone())
            .collect();
        self.runtime
            .orchestrator_project_last_checkpoint
            .retain(|project_id, _| live_orchestrator_projects.contains(project_id));

        let active_session = if matches!(self.ui.input_target, InputTarget::Agent) {
            self.selected_session().map(|s| s.id.clone())
        } else {
            None
        };
        let busy_markers = self.config.amq.inject.busy_markers.clone();
        let busy_scan_lines = self.config.amq.inject.busy_scan_lines.max(1);
        let startup_grace = Duration::from_millis(self.config.amq.inject.startup_grace_ms);

        for session_id in orchestrator_ids {
            let Some(target) = self.orchestrator_target_for_session(&session_id) else {
                continue;
            };
            let peers = self.orchestrator_peers_for_session(&session_id, &target.project_id);
            let needs_startup_policy = !self
                .runtime
                .orchestrator_policy_injected
                .contains(&session_id)
                && provider_needs_pty_orchestrator_policy(&target.provider);

            let prompt = if needs_startup_policy {
                let first_seen = self
                    .runtime
                    .orchestrator_last_nudged
                    .entry(session_id.clone())
                    .or_insert(now);
                if now.duration_since(*first_seen) < startup_grace {
                    continue;
                }
                build_orchestrator_startup_policy_prompt(&target.startup_policy, &peers)
            } else {
                if !self
                    .runtime
                    .orchestrator_policy_injected
                    .contains(&session_id)
                {
                    self.runtime
                        .orchestrator_policy_injected
                        .insert(session_id.clone());
                }
                let Some(last_nudged) = self
                    .runtime
                    .orchestrator_last_nudged
                    .get(&session_id)
                    .copied()
                else {
                    self.runtime
                        .orchestrator_last_nudged
                        .insert(session_id, now);
                    continue;
                };
                if now.duration_since(last_nudged) < poll_interval {
                    continue;
                }
                if self
                    .runtime
                    .orchestrator_project_last_checkpoint
                    .get(&target.project_id)
                    .is_some_and(|last_checkpoint| {
                        now.duration_since(*last_checkpoint) < poll_interval
                    })
                {
                    continue;
                }
                if peers.is_empty() {
                    continue;
                }
                build_orchestrator_checkpoint_prompt(&peers)
            };

            if self.runtime.watch_pending_enters.contains_key(&session_id) {
                continue;
            }
            if self
                .runtime
                .amq_inject_pending
                .get(&target.receiver)
                .is_some_and(|q| !q.is_empty())
            {
                continue;
            }
            if self
                .runtime
                .amq_inject_cooldown_until
                .get(&session_id)
                .is_some_and(|until| now < *until)
            {
                continue;
            }
            if active_session.as_deref() == Some(session_id.as_str()) {
                let quiet = Duration::from_secs(self.config.amq.inject.active_session_quiet_secs);
                let last = self.runtime.last_user_keystroke.get(&session_id).copied();
                if crate::app::inject_runtime::should_hold_for_quiet_window(last, now, quiet) {
                    continue;
                }
            }
            let snapshot = match self.find_pty_handle(&session_id) {
                Some(handle) => handle.scan_recent_lines(busy_scan_lines),
                None => continue,
            };
            if crate::amq_inject::snapshot_busy_marker(&snapshot, &busy_markers).is_some() {
                continue;
            }

            let payload = self.inject_body_bytes_for_session(&session_id, &prompt);
            let write_result = self
                .find_pty_handle(&session_id)
                .map(|handle| handle.write_bytes(&payload));
            match write_result {
                Some(Ok(())) => {
                    self.runtime
                        .watch_pending_enters
                        .insert(session_id.clone(), now);
                    self.runtime
                        .orchestrator_last_nudged
                        .insert(session_id.clone(), now);
                    if needs_startup_policy {
                        self.runtime
                            .orchestrator_policy_injected
                            .insert(session_id.clone());
                    } else {
                        self.runtime
                            .orchestrator_project_last_checkpoint
                            .insert(target.project_id.clone(), now);
                    }
                    tracing::info!(
                        target: "dux::orchestrator",
                        session_id = %session_id,
                        project_id = %target.project_id,
                        provider = %target.provider,
                        peer_count = peers.len(),
                        startup_policy = needs_startup_policy,
                        "sent orchestrator watchdog prompt",
                    );
                }
                Some(Err(err)) => {
                    tracing::warn!(
                        target: "dux::orchestrator",
                        session_id = %session_id,
                        err = %err,
                        "orchestrator checkpoint body write failed",
                    );
                }
                None => {}
            }
        }
    }

    fn orchestrator_target_for_session(&self, session_id: &str) -> Option<OrchestratorTarget> {
        self.git
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|session| OrchestratorTarget {
                receiver: amq_receiver_for_session(session),
                provider: self.running_provider_for(session).as_str().to_string(),
                project_id: session.project_id.clone(),
                startup_policy: session
                    .settings
                    .effective_system_prompt()
                    .unwrap_or_else(|| crate::model::ORCHESTRATOR_SYSTEM_PROMPT.to_string()),
            })
    }

    fn orchestrator_peers_for_session(
        &self,
        orchestrator_id: &str,
        orchestrator_project_id: &str,
    ) -> Vec<OrchestratorPeer> {
        self.git
            .sessions
            .iter()
            .filter(|s| {
                is_orchestrator_checkpoint_peer(
                    &s.id,
                    &s.project_id,
                    s.settings.mode,
                    s.state.is_live(),
                    orchestrator_id,
                    orchestrator_project_id,
                )
            })
            .map(|s| OrchestratorPeer {
                handle: amq_receiver_for_session(s),
                label: s.title.clone().unwrap_or_else(|| s.branch_name.clone()),
                provider: self.running_provider_for(s).as_str().to_string(),
                mode: s.settings.mode,
                branch: s.branch_name.clone(),
                worktree: s.worktree_path.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_prompt_lists_handles_and_orchestration_rules() {
        let prompt = build_orchestrator_checkpoint_prompt(&[OrchestratorPeer {
            handle: "front-end-qa".to_string(),
            label: "QA".to_string(),
            provider: "codex".to_string(),
            mode: ContextMode::Worker,
            branch: "feature/qa".to_string(),
            worktree: "/tmp/Front-end-QA".to_string(),
        }]);

        assert!(prompt.contains("front-end-qa"));
        assert!(prompt.contains("Do not do their implementation work yourself"));
        assert!(prompt.contains("amq send --to <handle>"));
        assert!(prompt.contains("status, blockers, ETA"));
    }

    #[test]
    fn startup_policy_prompt_carries_policy_without_peers() {
        let prompt =
            build_orchestrator_startup_policy_prompt(crate::model::ORCHESTRATOR_SYSTEM_PROMPT, &[]);

        assert!(prompt.contains("Dux Orchestrator startup policy"));
        assert!(prompt.contains("Orchestrate only"));
        assert!(prompt.contains("There are no active worker agents yet"));
    }

    #[test]
    fn startup_policy_prompt_does_not_trigger_worker_poll() {
        let prompt = build_orchestrator_startup_policy_prompt(
            crate::model::ORCHESTRATOR_SYSTEM_PROMPT,
            &[OrchestratorPeer {
                handle: "front-end-qa".to_string(),
                label: "QA".to_string(),
                provider: "codex".to_string(),
                mode: ContextMode::Worker,
                branch: "feature/qa".to_string(),
                worktree: "/tmp/Front-end-QA".to_string(),
            }],
        );

        assert!(prompt.contains("do not poll them on startup"));
        assert!(!prompt.contains("amq send --to <handle>"));
        assert!(!prompt.contains("Dux Orchestrator checkpoint"));
    }

    #[test]
    fn checkpoint_peers_are_live_workers_in_same_project_only() {
        assert!(is_orchestrator_checkpoint_peer(
            "worker-1",
            "project-a",
            ContextMode::Worker,
            true,
            "orchestrator-1",
            "project-a",
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "worker-2",
            "project-b",
            ContextMode::Worker,
            true,
            "orchestrator-1",
            "project-a",
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "attended-1",
            "project-a",
            ContextMode::Attended,
            true,
            "orchestrator-1",
            "project-a",
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "worker-3",
            "project-a",
            ContextMode::Worker,
            false,
            "orchestrator-1",
            "project-a",
        ));
        assert!(!is_orchestrator_checkpoint_peer(
            "orchestrator-1",
            "project-a",
            ContextMode::Orchestrator,
            true,
            "orchestrator-1",
            "project-a",
        ));
    }

    #[test]
    fn codex_needs_pty_policy_but_claude_does_not() {
        assert!(provider_needs_pty_orchestrator_policy("codex"));
        assert!(provider_needs_pty_orchestrator_policy("gemini"));
        assert!(!provider_needs_pty_orchestrator_policy("claude"));
        assert!(!provider_needs_pty_orchestrator_policy("Claude"));
    }
}
