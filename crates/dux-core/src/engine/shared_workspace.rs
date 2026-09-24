//! Shared main-workspace mode (fork shared-workspace Phases 4 and 6): the
//! engine-side questions both surfaces ask about agents that run directly in
//! a registered project checkout.
//!
//! - Which create a project's "new agent" makes ([`Engine::new_agent_is_shared`]).
//! - Who else is already writing in a checkout, so a second writer is asked
//!   first ([`Engine::live_shared_writer`]).
//! - How many shared checkouts currently have several live writers, for the
//!   persistent badge ([`Engine::shared_multi_writer_summary`]).
//! - Whether a project may get the worktrees link in its root
//!   ([`Engine::project_link_allowed`]).

use crate::config::WorkspaceMode;
use crate::model::AgentSession;

use super::Engine;

/// A live shared writer summary: `writers` live shared agents spread over
/// `workspaces` checkouts that each have more than one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedMultiWriterSummary {
    pub writers: usize,
    pub workspaces: usize,
}

impl SharedMultiWriterSummary {
    /// The badge text. "Current store only": another dux home sharing the
    /// checkout is invisible from here, and the badge must not claim otherwise.
    pub fn badge(self) -> String {
        if self.workspaces == 1 {
            format!("CURRENT STORE ONLY \u{b7} {} LIVE WRITERS", self.writers)
        } else {
            format!(
                "CURRENT STORE ONLY \u{b7} {} LIVE WRITERS IN {} SHARED WORKSPACES",
                self.writers, self.workspaces
            )
        }
    }
}

impl Engine {
    /// Whether a fresh "new agent" for this project runs in its checkout
    /// (shared mode) rather than in a new worktree. Forks, PR agents and agents
    /// adopted from an existing worktree are always isolated; only the plain
    /// new-agent flow consults this.
    pub fn new_agent_is_shared(&self, project_id: &str) -> bool {
        self.config.workspace_mode_for_project_id(project_id) == WorkspaceMode::Shared
    }

    /// Whether any tab of this agent has a live provider.
    pub fn session_has_live_provider(&self, session_id: &str) -> bool {
        self.tab_ids_for_session(session_id)
            .iter()
            .any(|tab| self.providers.contains_key(tab))
    }

    /// Another shared agent already running in `checkout`, other than
    /// `exclude_session_id`. A second writer in one checkout is allowed, but
    /// only after the user confirms it (fork d0ce0afc): two agents editing the
    /// same files can overwrite each other's work.
    pub fn live_shared_writer(
        &self,
        checkout: &str,
        exclude_session_id: Option<&str>,
    ) -> Option<&AgentSession> {
        self.sessions.iter().find(|session| {
            session.shared_workspace()
                && exclude_session_id != Some(session.id.as_str())
                && crate::project_browser::same_directory(session.directory(), checkout)
                && self.session_has_live_provider(&session.id)
        })
    }

    /// Checkouts with more than one live shared writer, or `None` when there
    /// are none. Counts only this dux home's agents.
    pub fn shared_multi_writer_summary(&self) -> Option<SharedMultiWriterSummary> {
        let mut counts: Vec<(std::path::PathBuf, usize)> = Vec::new();
        for session in self
            .sessions
            .iter()
            .filter(|s| s.shared_workspace() && self.session_has_live_provider(&s.id))
        {
            let key = crate::project_browser::canonical_or_original(std::path::Path::new(
                session.directory(),
            ));
            match counts.iter_mut().find(|(path, _)| *path == key) {
                Some((_, count)) => *count += 1,
                None => counts.push((key, 1)),
            }
        }
        let crowded: Vec<usize> = counts
            .into_iter()
            .map(|(_, count)| count)
            .filter(|count| *count > 1)
            .collect();
        (!crowded.is_empty()).then(|| SharedMultiWriterSummary {
            writers: crowded.iter().sum(),
            workspaces: crowded.len(),
        })
    }

    /// Whether dux may create the project-root link to its managed worktrees
    /// (fork 1d69de16) for `project_id`. Only when the project has at least one
    /// isolated (non-shared) agent: a project used only in shared mode has no
    /// worktrees to expose, and its checkout must not gain a link or an
    /// `.git/info/exclude` entry it never asked for (fork
    /// `shared_only_project_skips_link_but_isolated_fork_creates_it`).
    pub fn project_link_allowed(&self, project_id: &str) -> bool {
        project_link_allowed(&self.sessions, project_id)
    }
}

/// [`Engine::project_link_allowed`] over a plain session list, for callers
/// that hold a snapshot instead of the engine.
pub fn project_link_allowed(sessions: &[AgentSession], project_id: &str) -> bool {
    sessions
        .iter()
        .any(|session| session.project_id() == Some(project_id) && !session.shared_workspace())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::ids::TabId;
    use crate::pty::PtyClient;

    fn shared_session(id: &str, checkout: &str) -> AgentSession {
        let mut session = sample_session(id, "p1", "main");
        session.shared_workspace = true;
        session
            .workspace
            .as_managed_mut()
            .expect("managed")
            .worktree_path = checkout.to_string();
        session
    }

    fn make_live(engine: &mut Engine, session_id: &str) {
        let slot = engine
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .expect("session")
            .slot_tab_id()
            .to_owned();
        let client =
            PtyClient::spawn_with_env("cat", &[], std::path::Path::new("."), 24, 80, 100, &[])
                .expect("spawn cat");
        engine.providers.insert(TabId::new(slot.as_str()), client);
    }

    #[test]
    fn new_agent_mode_follows_project_override_then_global() {
        let (mut engine, _tmp) = test_engine();
        assert!(
            !engine.new_agent_is_shared("p1"),
            "legacy config keeps worktrees"
        );
        engine.config.workspace = Some(crate::config::WorkspaceConfig::default());
        assert!(
            engine.new_agent_is_shared("p1"),
            "fresh config defaults to shared"
        );
        engine.config.workspace = None;
        engine.config.projects.push(crate::config::ProjectConfig {
            id: "p1".to_string(),
            path: "/p1".to_string(),
            name: None,
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            workspace_mode: Some(WorkspaceMode::Shared),
        });
        assert!(engine.new_agent_is_shared("p1"));
    }

    #[test]
    fn live_shared_writer_finds_only_another_live_shared_agent_in_that_checkout() {
        let (mut engine, tmp) = test_engine();
        let checkout = tmp.path().join("repo");
        std::fs::create_dir_all(&checkout).unwrap();
        let checkout = checkout.to_string_lossy().to_string();
        engine.sessions.push(shared_session("live", &checkout));
        engine.sessions.push(shared_session("idle", &checkout));
        assert!(engine.live_shared_writer(&checkout, None).is_none());

        make_live(&mut engine, "live");
        assert_eq!(
            engine
                .live_shared_writer(&checkout, Some("idle"))
                .map(|s| s.id.as_str()),
            Some("live")
        );
        assert!(
            engine.live_shared_writer(&checkout, Some("live")).is_none(),
            "an agent is not its own second writer"
        );
        assert!(engine.shared_multi_writer_summary().is_none());

        make_live(&mut engine, "idle");
        let summary = engine.shared_multi_writer_summary().expect("two writers");
        assert_eq!(
            summary,
            SharedMultiWriterSummary {
                writers: 2,
                workspaces: 1
            }
        );
        assert_eq!(summary.badge(), "CURRENT STORE ONLY \u{b7} 2 LIVE WRITERS");
        engine.shutdown_ptys(std::time::Duration::ZERO);
    }

    /// Fork `begin_delete_session_never_removes_shared_workspace`: a delete
    /// that asks for the worktree is refused out loud for a shared agent, and
    /// a plain delete removes only dux's record, leaving the checkout alone.
    #[test]
    fn begin_delete_session_never_removes_shared_workspace() {
        use crate::engine::BeginDeleteSessionOutcome;
        let (mut engine, tmp) = test_engine();
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        let sentinel = checkout.join("uncommitted.txt");
        std::fs::write(&sentinel, "keep me").unwrap();
        let checkout_str = checkout.to_string_lossy().to_string();
        engine.projects.push(sample_project("p1", &checkout_str));
        let session = shared_session("shared", &checkout_str);
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine.begin_delete_session("shared", true, None);
        let BeginDeleteSessionOutcome::Refused { message } = outcome else {
            panic!("a worktree-removing delete of a shared agent must be refused, got {outcome:?}");
        };
        assert!(message.contains("shared project checkout"), "{message}");
        assert!(engine.pending_deletions.is_empty());
        assert!(
            engine.do_delete_session("shared", true, None).is_err(),
            "the synchronous path refuses the same way"
        );

        let outcome = engine.begin_delete_session("shared", false, None);
        assert!(
            matches!(outcome, BeginDeleteSessionOutcome::Inline { .. }),
            "a record-only delete takes the inline path, got {outcome:?}"
        );
        engine.finish_delete_session("shared").unwrap();
        assert!(engine.pending_deletions.is_empty());
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep me");
        assert!(engine.session_store.load_sessions().unwrap().is_empty());
        assert_eq!(
            engine
                .session_store
                .load_sessions_including_deleted()
                .unwrap()
                .len(),
            1,
            "the tombstone keeps the handle reserved"
        );
    }

    /// Fork `shared_session_rejects_real_branch_rename` and
    /// `shared_session_rename_changes_only_display_title`.
    #[test]
    fn shared_session_rejects_real_branch_rename_but_renames_the_title() {
        use crate::engine::{BranchRenamePlan, BranchRenameRejection};
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(shared_session("shared", "/repo"));
        let handle = engine.sessions[0].agent_handle().to_string();

        assert_eq!(
            engine.prepare_branch_rename("shared", "display-title", true),
            BranchRenamePlan::Rejected(BranchRenameRejection::SharedWorkspaceBranch)
        );
        assert_eq!(engine.sessions[0].title.as_deref(), Some("shared-title"));

        let plan = engine.prepare_branch_rename("shared", "display-title", false);
        assert!(
            matches!(plan, BranchRenamePlan::TitleWritten { .. }),
            "{plan:?}"
        );
        assert_eq!(engine.sessions[0].title.as_deref(), Some("display-title"));
        assert_eq!(engine.sessions[0].branch_name(), Some("main"));
        assert_eq!(engine.sessions[0].agent_handle(), handle);
    }

    fn reopenable(engine: &mut Engine, tmp: &std::path::Path) {
        engine.config.ui.auto_reopen_agents = true;
        let checkout = tmp.join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        let mut session = shared_session("shared", &checkout.to_string_lossy());
        session.desired_running = true;
        session.auto_reopen_enabled = true;
        engine.sessions.push(session);
    }

    #[test]
    fn startup_auto_resume_excludes_shared_sessions_by_default() {
        let (mut engine, tmp) = test_engine();
        reopenable(&mut engine, tmp.path());
        assert!(!engine.config.auto_resume_shared());
        assert!(engine.auto_reopen_candidates().is_empty());
    }

    #[test]
    fn startup_auto_resume_includes_shared_sessions_when_opted_in() {
        let (mut engine, tmp) = test_engine();
        reopenable(&mut engine, tmp.path());
        engine.config.workspace = Some(crate::config::WorkspaceConfig {
            default_mode: WorkspaceMode::Shared,
            auto_resume_shared: true,
        });
        let ids: Vec<String> = engine
            .auto_reopen_candidates()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["shared".to_string()]);
    }

    /// Several writers in one checkout is the point of shared mode, so the
    /// worktree-conflict auto-detach never fires when either side is shared.
    #[test]
    fn conflict_detach_is_noop_when_either_session_is_shared() {
        for shared_id in ["s1", "s2"] {
            let (mut engine, _tmp) = test_engine();
            let mut s1 = sample_session("s1", "p1", "main");
            let mut s2 = sample_session("s2", "p1", "main");
            for session in [&mut s1, &mut s2] {
                session.workspace.as_managed_mut().unwrap().worktree_path = "/tmp/wt/shared".into();
            }
            if shared_id == "s1" {
                s1.shared_workspace = true;
            } else {
                s2.shared_workspace = true;
            }
            engine.sessions.push(s1);
            engine.sessions.push(s2);
            make_live(&mut engine, "s1");

            assert!(
                engine
                    .detach_conflicting_worktree_session("/tmp/wt/shared", "s2")
                    .is_none()
            );
            assert!(engine.session_has_live_provider("s1"));
            engine.shutdown_ptys(std::time::Duration::ZERO);
        }
    }

    /// The web create honors the project's mode, and refuses a second live
    /// writer because the browser has no consent dialog.
    #[test]
    fn wire_create_agent_follows_shared_mode_and_refuses_a_second_writer() {
        use crate::engine::Command;
        use crate::wire::WireCommand;
        use crate::worker::CreateAgentRequest;
        let (mut engine, tmp) = test_engine();
        let checkout = tmp.path().join("checkout");
        std::fs::create_dir_all(&checkout).unwrap();
        let checkout = checkout.to_string_lossy().to_string();
        engine.projects.push(sample_project("p1", &checkout));
        engine.config.workspace = Some(crate::config::WorkspaceConfig::default());
        let create = || WireCommand::CreateAgent {
            project_id: "p1".to_string(),
            name: "writer".to_string(),
            copy_uncommitted_changes: None,
            use_existing_branch: false,
        };

        let command = engine.wire_to_command(create()).expect("shared create");
        let Command::DispatchCreateAgentRequest { request, .. } = command else {
            panic!("expected a create dispatch");
        };
        assert!(matches!(
            *request,
            CreateAgentRequest::SharedWorkspace { ref custom_name, .. }
                if custom_name.as_deref() == Some("writer")
        ));

        engine.sessions.push(shared_session("live", &checkout));
        make_live(&mut engine, "live");
        let Err(err) = engine.wire_to_command(create()) else {
            panic!("a second writer is refused on the web");
        };
        assert!(format!("{err:#}").contains("already running in the shared checkout"));
        engine.shutdown_ptys(std::time::Duration::ZERO);
    }

    #[test]
    fn shared_only_project_skips_link_but_isolated_fork_creates_it() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(shared_session("shared", "/repo"));
        assert!(!engine.project_link_allowed("p1"));

        // A fork is an isolated agent even under a shared project default.
        engine.sessions.push(sample_session("fork", "p1", "fork"));
        assert!(engine.project_link_allowed("p1"));
        assert!(!engine.project_link_allowed("other"));
    }

    /// Fork `multi_writer_badge_is_derived_and_recomputes_when_writer_exits`:
    /// the badge is derived from live state rather than stored, so an isolated
    /// agent in the same directory never counts, and when a shared writer
    /// exits (the exit sweep drops its tab through `clear_tab_runtime`) the
    /// badge goes away with no other bookkeeping.
    #[test]
    fn multi_writer_badge_is_derived_and_recomputes_when_writer_exits() {
        let (mut engine, tmp) = test_engine();
        let checkout = tmp.path().join("project");
        std::fs::create_dir_all(&checkout).unwrap();
        let checkout = checkout.to_string_lossy().to_string();
        engine.sessions.push(shared_session("first", &checkout));
        engine.sessions.push(shared_session("second", &checkout));
        let mut isolated = shared_session("isolated", &checkout);
        isolated.shared_workspace = false;
        engine.sessions.push(isolated);

        make_live(&mut engine, "first");
        assert_eq!(engine.shared_multi_writer_summary(), None);
        make_live(&mut engine, "isolated");
        assert_eq!(
            engine.shared_multi_writer_summary(),
            None,
            "an isolated agent in the same directory is not a shared writer"
        );
        make_live(&mut engine, "second");
        assert_eq!(
            engine.shared_multi_writer_summary().map(|s| s.badge()),
            Some("CURRENT STORE ONLY \u{b7} 2 LIVE WRITERS".to_string())
        );

        let second_slot = engine
            .sessions
            .iter()
            .find(|s| s.id == "second")
            .unwrap()
            .slot_tab_id()
            .to_owned();
        engine.clear_tab_runtime(&second_slot);
        assert_eq!(engine.shared_multi_writer_summary(), None);
        engine.shutdown_ptys(std::time::Duration::ZERO);
    }
}
