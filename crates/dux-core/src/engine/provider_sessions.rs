//! Resume by provider session id, and throttled startup relaunch (fork
//! a38187f3, eb2d2eee, 478c9e3c, 07d9b0ba, 8eb821e6).
//!
//! Upstream resumes with each provider's "latest conversation in this
//! directory" flag (`resume_args`). That picks by recency, not by agent, so it
//! hands the wrong conversation back as soon as two conversations share a
//! directory. This module layers the fork's targeted resume on top:
//!
//! - [`Engine::provider_session_launch`] decides, per launch, whether to resume
//!   a recorded id (preferred), capture the id of a fresh conversation, or do
//!   exactly what upstream did. It only ever UPGRADES a launch upstream already
//!   decided: the collision rule in `tab_resume_decision` still applies first.
//! - Captured ids come back as `WorkerEvent::ProviderSessionCaptured` and are
//!   persisted here, through the store's dedicated setters.
//! - Startup relaunches (upstream's auto-reopen, and the fork's
//!   `auto_resume_on_start`) go through one candidate filter and one throttle.

use std::path::Path;
use std::time::Instant;

use crate::auto_resume::QueuedStartupLaunch;
use crate::engine::events::{EventReaction, StatusUpdate};
use crate::engine::{Command, Engine, InFlightKey};
use crate::ids::TabIdRef;
use crate::model::{AgentSession, ProviderKind};
use crate::resume_recovery::{self, ProviderSessionLaunch, ResumeIdInput};
use crate::worker::{AgentLaunchKind, WorkerEvent};

impl Engine {
    /// How a launch of `provider` on `tab_id` reaches its conversation, given
    /// upstream's own `resume` decision for it.
    ///
    /// - A resume-eligible launch resumes by id when dux knows one, and falls
    ///   back to upstream's resume-latest flag when it does not.
    /// - A launch that is NOT allowed to resume (upstream said no because a
    ///   sibling tab of the same provider is live, or the caller asked for a
    ///   fresh start) never resumes by id either, and captures the id of the
    ///   fresh conversation it starts only from the session-slot tab, so an
    ///   extra tab never replaces the agent's recorded conversation.
    pub fn provider_session_launch(
        &self,
        session: &AgentSession,
        tab_id: &TabIdRef,
        provider: &ProviderKind,
        requested: bool,
        resume: bool,
    ) -> ProviderSessionLaunch {
        let config = crate::config::provider_config(&self.config, provider);
        let slot = self.is_slot_tab(session, tab_id);
        if requested && slot && !self.tab_has_live_same_provider_sibling(session, tab_id, provider)
        {
            let stored = self
                .session_store
                .provider_session_id(&session.id, provider.as_str())
                .ok()
                .flatten();
            let directory = Path::new(session.directory());
            let input = ResumeIdInput {
                provider: provider.as_str(),
                config: &config,
                stored_id: stored.as_deref(),
                shared: resume_recovery::session_is_shared(session),
                directory,
                started: session.has_started_provider(provider),
                directory_has_sibling: self.sessions.iter().any(|other| {
                    other.id != session.id
                        && other.provider == *provider
                        && other.directory() == session.directory()
                }),
            };
            if let Some(id) = resume_recovery::plan_resume_id(
                &input,
                resume_recovery::claude_resume_target_exists,
                resume_recovery::resolve_latest_jcode_session_from_home,
            ) {
                return ProviderSessionLaunch::ResumeId(id);
            }
        }
        if resume || !slot {
            return ProviderSessionLaunch::Plain;
        }
        resume_recovery::fresh_capture_for(provider.as_str(), &config)
    }

    /// Whether another tab of `session` is running (or launching) `provider`.
    /// The same collision `tab_resume_decision` guards, asked on its own so a
    /// provider with no `resume_args` (jcode) is still kept from resuming the
    /// conversation a live sibling already holds.
    fn tab_has_live_same_provider_sibling(
        &self,
        session: &AgentSession,
        tab_id: &TabIdRef,
        provider: &ProviderKind,
    ) -> bool {
        self.tab_ids_for_session(&session.id).into_iter().any(|id| {
            id != tab_id
                && self.tab_is_live(id.as_ref_id())
                && self.tab_running_provider(session, &id) == *provider
        })
    }

    /// Record a captured provider conversation id, or report why capture
    /// failed closed. The id is written through the store's dedicated setter,
    /// never through `upsert_session`, so status churn cannot erase it.
    pub(crate) fn process_provider_session_captured(
        &mut self,
        session_id: &str,
        provider: &str,
        result: Result<String, String>,
    ) -> EventReaction {
        match result {
            Ok(provider_session_id) => {
                match self.session_store.set_provider_session_id(
                    session_id,
                    provider,
                    &provider_session_id,
                ) {
                    Ok(()) => {
                        crate::logger::info(&format!(
                            "recorded the {provider} conversation id for agent {session_id}"
                        ));
                        EventReaction::Nothing
                    }
                    // The agent was deleted while its capture ran: nothing to
                    // resume, so nothing to tell anyone.
                    Err(_) if self.session_by_id(session_id).is_none() => EventReaction::Nothing,
                    Err(err) => EventReaction::Status(StatusUpdate::warning(format!(
                        "Could not record the {provider} conversation for this agent: {err:#}. \
                         Its next start resumes the most recent conversation in its directory instead."
                    ))),
                }
            }
            Err(err) => {
                crate::logger::warn(&format!(
                    "{provider} conversation capture for agent {session_id} failed closed: {err}"
                ));
                if provider == "codex" {
                    EventReaction::Status(StatusUpdate::warning(format!(
                        "Could not identify this agent's Codex conversation: {err}. \
                         Other fresh Codex agents in this directory go uncaptured until dux restarts."
                    )))
                } else {
                    EventReaction::Nothing
                }
            }
        }
    }

    /// Start the one-time recovery of Claude and Codex histories stranded under
    /// old per-agent worktrees (fork 36568df7), off the engine thread. Returns
    /// whether a worker was started: with nothing to recover it is not, so a
    /// normal startup never scans the provider directories at all.
    pub fn dispatch_resume_recovery(&self) -> bool {
        if !resume_recovery::recovery_has_candidates(
            &self.sessions,
            &self.projects,
            &self.session_store,
        ) {
            return false;
        }
        let sessions = self.sessions.clone();
        let projects = self.projects.clone();
        let worktrees_root = self.paths.worktrees_root.clone();
        let db_path = self.paths.sessions_db_path.clone();
        let tx = self.worker_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("provider-session-recovery".to_string())
            .spawn(move || {
                let result = crate::storage::SessionStore::open(&db_path)
                    .and_then(|store| {
                        let roots = resume_recovery::ProviderDataRoots::from_home()?;
                        resume_recovery::recover_stranded_histories(
                            &sessions,
                            &projects,
                            &worktrees_root,
                            &roots,
                            &store,
                        )
                    })
                    .map_err(|err| format!("{err:#}"));
                let _ = tx.send(WorkerEvent::ResumeRecoveryCompleted(result));
            });
        spawned.is_ok()
    }

    pub(crate) fn process_resume_recovery_completed(
        &mut self,
        result: Result<resume_recovery::RecoveryReport, String>,
    ) -> EventReaction {
        // Whatever the outcome, startup launches waiting for it may go now.
        self.startup_launches.release();
        match result {
            Ok(report) => {
                crate::logger::info(&format!(
                    "provider history recovery: {} agent(s) recovered, {} artifact(s) copied, {} warning(s)",
                    report.updates.len(),
                    report.copied_artifacts,
                    report.warnings.len(),
                ));
                for warning in &report.warnings {
                    crate::logger::warn(warning);
                }
                match report.warnings.first() {
                    Some(first) => EventReaction::Status(StatusUpdate::warning(first.clone())),
                    None => EventReaction::Nothing,
                }
            }
            Err(err) => {
                crate::logger::warn(&format!("provider history recovery failed: {err}"));
                EventReaction::Status(StatusUpdate::warning(format!(
                    "Recovering earlier provider conversations did not complete: {err}"
                )))
            }
        }
    }

    /// The agents a startup pass relaunches, in order.
    ///
    /// Upstream's `auto_reopen_candidates` is the base, so every one of its
    /// rules holds here too (including skipping agents a reload adopted, which
    /// are already live). `defaults.auto_resume_on_start` widens the set to
    /// every agent with an existing directory and no live tab, regardless of
    /// the reopen intent and opt-ins, like the fork did. Either way, agents
    /// whose directory is older than `[auto_resume].stale_days` are dropped.
    ///
    /// Shared-directory agents join the widened set only with
    /// `[workspace].auto_resume_shared` (8eb821e6): a boot must not fire every
    /// shared agent into the real checkout unasked.
    pub fn startup_launch_candidates(&self) -> Vec<AgentSession> {
        let mut candidates = self.auto_reopen_candidates();
        if self.config.defaults.auto_resume_on_start {
            let include_shared = self.config.auto_resume_shared();
            for session in &self.sessions {
                if candidates.iter().any(|c| c.id == session.id) {
                    continue;
                }
                if (include_shared || !resume_recovery::session_is_shared(session))
                    && Path::new(session.directory()).exists()
                    && !self.any_tab_active(&session.id)
                {
                    candidates.push(session.clone());
                }
            }
        }
        let stale_days = self.config.auto_resume.stale_days;
        candidates.retain(|session| {
            let stale = crate::auto_resume::is_stale(Path::new(session.directory()), stale_days);
            if stale {
                crate::logger::info(&format!(
                    "startup relaunch skipped for {}: its directory is older than {stale_days} days",
                    session.id
                ));
            }
            !stale
        });
        candidates
    }

    /// Queue every startup candidate behind the `[auto_resume]` throttle.
    /// Returns how many were queued.
    ///
    /// `recovering` says a history recovery was just dispatched. A shared
    /// agent has no resume-latest fallback, so launching it before recovery
    /// records its id would start it fresh and lose the conversation; the
    /// queue is then held until `ResumeRecoveryCompleted`, as the fork ran
    /// auto-resume only after recovery. Unshared agents lose nothing by going
    /// first, but one queue keeps one order, so they wait too.
    pub fn queue_startup_launches(&mut self, recovering: bool) -> usize {
        let candidates = self.startup_launch_candidates();
        for session in &candidates {
            self.startup_launches.enqueue(QueuedStartupLaunch {
                session_id: session.id.clone(),
            });
        }
        if recovering && candidates.iter().any(resume_recovery::session_is_shared) {
            self.startup_launches.hold();
        }
        if !candidates.is_empty() {
            crate::logger::info(&format!(
                "startup relaunch: {} agent(s) queued (concurrency={}, stagger={}ms)",
                candidates.len(),
                self.config.auto_resume.concurrency,
                self.config.auto_resume.stagger_ms,
            ));
        }
        candidates.len()
    }

    /// Dispatch the queued startup launches the throttle allows now. Called
    /// every tick by both surfaces; a no-op once the queue is empty. Every
    /// launch goes through the `DispatchAgentLaunch` chokepoint as a
    /// `StartupAutoReopen`, exactly as upstream's unthrottled pass did.
    pub fn pump_startup_launches(&mut self, pty_size: (u16, u16)) -> Vec<EventReaction> {
        if self.startup_launches.is_empty() {
            return Vec::new();
        }
        let in_flight = self.in_flight.clone();
        self.startup_launches
            .settle(|tab| in_flight.contains(&InFlightKey::AgentLaunch(tab.clone())));
        let mut reactions = Vec::new();
        let cfg = self.config.auto_resume.clone();
        loop {
            let now = Instant::now();
            let Some(next) = self.startup_launches.next_ready(&cfg, now) else {
                break;
            };
            // Re-check at dispatch time: the agent may have been deleted,
            // started by hand, or adopted since it was queued.
            let Some(session) = self.session_by_id(&next.session_id).cloned() else {
                continue;
            };
            if self.any_tab_active(&session.id) {
                continue;
            }
            let tab_id = session.slot_tab_id().to_owned();
            let request = self.build_agent_launch_request(
                session,
                true,
                pty_size,
                AgentLaunchKind::StartupAutoReopen,
            );
            match self.apply(Command::DispatchAgentLaunch {
                request: Box::new(request),
            }) {
                Ok(reaction) => {
                    let launched = matches!(
                        &reaction,
                        EventReaction::DispatchAgentLaunchView(view) if view.launched
                    );
                    self.startup_launches
                        .dispatched(launched.then_some(tab_id), now);
                    reactions.push(reaction);
                }
                Err(err) => {
                    self.startup_launches.dispatched(None, now);
                    crate::logger::info(&format!(
                        "startup relaunch of {} failed to dispatch: {err:#}",
                        next.session_id
                    ));
                }
            }
        }
        reactions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};

    fn session_in(dir: &Path, id: &str, provider: &str) -> AgentSession {
        let mut session = sample_session(id, "p1", id);
        session.provider = ProviderKind::new(provider);
        if let Some(managed) = session.workspace.as_managed_mut() {
            managed.worktree_path = dir.to_string_lossy().to_string();
        }
        session
    }

    /// A stored id is preferred over upstream's resume-latest flag, and the
    /// resume-latest flag stays the fallback when no id is known.
    #[test]
    fn stored_id_beats_resume_latest_and_latest_stays_the_fallback() {
        let (mut engine, tmp) = test_engine();
        let mut session = session_in(tmp.path(), "s1", "codex");
        session.started_providers = vec!["codex".to_string()];
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session.clone());

        let request = engine.build_agent_launch_request(
            session.clone(),
            true,
            (24, 80),
            AgentLaunchKind::StartupAutoReopen,
        );
        assert!(request.resume, "upstream still decides resume-latest");
        assert_eq!(request.provider_session, ProviderSessionLaunch::Plain);

        let id = uuid::Uuid::new_v4().to_string();
        engine
            .session_store
            .set_provider_session_id("s1", "codex", &id)
            .unwrap();
        let request = engine.build_agent_launch_request(
            session,
            true,
            (24, 80),
            AgentLaunchKind::StartupAutoReopen,
        );
        assert_eq!(
            request.provider_session,
            ProviderSessionLaunch::ResumeId(id)
        );
    }

    /// A fresh slot launch of a capturing provider asks the worker to capture
    /// its conversation id; an extra tab and a resuming launch never do.
    #[test]
    fn fresh_slot_launch_captures_and_extra_tabs_do_not() {
        let (mut engine, tmp) = test_engine();
        let session = session_in(tmp.path(), "s1", "claude");
        engine.sessions.push(session.clone());
        let slot = session.slot_tab_id().to_owned();

        assert_eq!(
            engine.provider_session_launch(&session, &slot, &session.provider, false, false),
            ProviderSessionLaunch::CaptureClaude
        );
        assert_eq!(
            engine.provider_session_launch(
                &session,
                TabIdRef::new("extra-tab"),
                &session.provider,
                false,
                false
            ),
            ProviderSessionLaunch::Plain
        );
    }

    /// A tab launched beside a live tab of the same provider must not resume by
    /// id: the conversation belongs to the live one.
    #[test]
    fn a_live_same_provider_sibling_blocks_resume_by_id() {
        let (mut engine, tmp) = test_engine();
        let mut session = session_in(tmp.path(), "s1", "jcode");
        session.started_providers = vec!["jcode".to_string()];
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session.clone());
        engine
            .session_store
            .set_provider_session_id("s1", "jcode", "session_cactus_1_ab")
            .unwrap();
        let slot = session.slot_tab_id().to_owned();
        assert!(
            engine
                .provider_session_launch(&session, &slot, &session.provider, true, false)
                .is_resume_id(),
            "alone, jcode resumes by id even though it has no resume_args"
        );

        engine.agent_tabs.insert(
            crate::ids::TabId::new("tab-2"),
            crate::engine::test_support::sample_tab("tab-2", "s1", "jcode", 1),
        );
        engine
            .in_flight
            .insert(InFlightKey::AgentLaunch(crate::ids::TabId::new("tab-2")));
        assert_eq!(
            engine.provider_session_launch(&session, &slot, &session.provider, true, false),
            ProviderSessionLaunch::Plain
        );
    }

    #[test]
    fn captured_id_is_persisted_through_the_dedicated_setter() {
        let (mut engine, tmp) = test_engine();
        let session = session_in(tmp.path(), "s1", "claude");
        engine.session_store.create_session(&session).unwrap();
        engine.sessions.push(session);
        let id = uuid::Uuid::new_v4().to_string();

        let reaction = engine.process_worker_event(WorkerEvent::ProviderSessionCaptured {
            session_id: "s1".to_string(),
            provider: "claude".to_string(),
            result: Ok(id.clone()),
        });
        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(
            engine
                .session_store
                .provider_session_id("s1", "claude")
                .unwrap(),
            Some(id)
        );
    }

    /// `auto_resume_on_start` widens upstream's candidate set to every agent
    /// with a directory, but never to one that is already live (a reload's
    /// adopted agents), and stale directories are dropped either way.
    #[test]
    fn auto_resume_on_start_widens_candidates_but_skips_live_and_stale_agents() {
        let (mut engine, tmp) = test_engine();
        let dir = tmp.path().join("agent");
        std::fs::create_dir_all(&dir).unwrap();
        let mut idle = session_in(&dir, "idle", "claude");
        idle.desired_running = false;
        let mut live = session_in(&dir, "live", "claude");
        live.desired_running = false;
        engine.sessions.push(idle);
        engine.sessions.push(live.clone());
        engine
            .in_flight
            .insert(InFlightKey::AgentLaunch(live.slot_tab_id().to_owned()));

        assert!(
            engine.startup_launch_candidates().is_empty(),
            "off by default"
        );

        engine.config.defaults.auto_resume_on_start = true;
        let ids: Vec<String> = engine
            .startup_launch_candidates()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["idle".to_string()]);

        // Backdate the directory past the stale threshold.
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        std::fs::File::open(&dir)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(engine.startup_launch_candidates().is_empty());
        engine.config.auto_resume.stale_days = 0;
        assert_eq!(engine.startup_launch_candidates().len(), 1);
    }

    fn startup_fixture() -> (Engine, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        let dir = tmp.path().join("checkout");
        std::fs::create_dir_all(&dir).unwrap();
        let mut shared = session_in(&dir, "shared", "claude");
        shared.shared_workspace = true;
        shared.desired_running = false;
        let mut worktree = session_in(&dir, "worktree", "claude");
        worktree.desired_running = false;
        engine.sessions.push(shared);
        engine.sessions.push(worktree);
        engine.config.defaults.auto_resume_on_start = true;
        (engine, tmp)
    }

    fn candidate_ids(engine: &Engine) -> Vec<String> {
        engine
            .startup_launch_candidates()
            .into_iter()
            .map(|s| s.id)
            .collect()
    }

    /// 8eb821e6: a boot must not fire every shared agent into the real
    /// checkout unasked, so shared agents are excluded by default.
    #[test]
    fn startup_auto_resume_excludes_shared_sessions_by_default() {
        let (engine, _tmp) = startup_fixture();
        assert_eq!(candidate_ids(&engine), vec!["worktree".to_string()]);
    }

    /// `[workspace].auto_resume_shared = true` opts shared agents back in.
    #[test]
    fn startup_auto_resume_includes_shared_sessions_when_opted_in() {
        let (mut engine, _tmp) = startup_fixture();
        let mut workspace = engine.config.workspace.clone().unwrap_or_default();
        workspace.auto_resume_shared = true;
        engine.config.workspace = Some(workspace);
        assert_eq!(
            candidate_ids(&engine),
            vec!["shared".to_string(), "worktree".to_string()]
        );
    }

    /// A shared agent waiting for startup is held until history recovery
    /// reports, because launching it first would start it fresh (it has no
    /// resume-latest fallback) and lose the conversation recovery would find.
    #[test]
    fn startup_queue_waits_for_recovery_when_a_shared_agent_is_queued() {
        let (mut engine, _tmp) = startup_fixture();
        let mut workspace = engine.config.workspace.clone().unwrap_or_default();
        workspace.auto_resume_shared = true;
        engine.config.workspace = Some(workspace);
        engine.config.auto_resume.stagger_ms = 0;
        assert_eq!(engine.queue_startup_launches(true), 2);
        assert!(engine.pump_startup_launches((24, 80)).is_empty());
        engine.process_worker_event(WorkerEvent::ResumeRecoveryCompleted(Ok(Default::default())));
        assert!(!engine.startup_launches.is_held());
    }

    /// The pump never has more than `concurrency` startup launches in flight.
    #[test]
    fn pump_respects_concurrency() {
        let (mut engine, tmp) = test_engine();
        engine.config.defaults.auto_resume_on_start = true;
        engine.config.auto_resume.concurrency = 1;
        engine.config.auto_resume.stagger_ms = 0;
        // A provider command that does not exist fails fast in the worker, so
        // no real process is spawned.
        for id in ["a", "b"] {
            let dir = tmp.path().join(id);
            std::fs::create_dir_all(&dir).unwrap();
            let mut session = session_in(&dir, id, "definitely-missing-provider");
            session.desired_running = false;
            engine.sessions.push(session);
        }
        assert_eq!(engine.queue_startup_launches(false), 2);
        let first = engine.pump_startup_launches((24, 80));
        assert_eq!(first.len(), 1, "one slot, one launch");
        assert_eq!(engine.startup_launches.pending_len(), 1);
        assert!(engine.pump_startup_launches((24, 80)).is_empty());
    }
}
