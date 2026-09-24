//! The `dux-worktrees` explorer link in each project checkout (port of fork
//! 1d69de16 with the 18a13536 hardening).
//!
//! The git and filesystem work is [`crate::git::ensure_project_worktrees_link`];
//! this decides WHICH projects get a link and runs it off the engine thread, as
//! every git call must. Failures are logged, never shown: the link is a
//! convenience, and a project checkout that refuses it (a real `dux-worktrees`
//! directory, a read-only repo) must not nag on every start.

use std::path::PathBuf;

use super::Engine;
use crate::model::Project;

impl Engine {
    /// Whether `project` should carry the link: it exists on disk and at least
    /// one of its agents runs in a managed worktree. A project whose agents all
    /// run in the shared checkout has no worktrees to show.
    pub fn project_wants_worktrees_link(&self, project: &Project) -> bool {
        // INTEGRATION: shared-workspace link predicate (evergreen/pig). Until
        // that workstream publishes a named predicate, "has a non-shared agent
        // in a managed worktree" is computed here from evergreen's
        // AgentSession::shared_workspace().
        !project.path_missing
            && self.sessions.iter().any(|session| {
                session.project_id() == Some(project.id.as_str())
                    && !session.is_deleted()
                    && !session.shared_workspace()
                    && session.managed_worktree().is_some()
            })
    }

    /// Ensure the link for every eligible project, on one background worker.
    pub fn spawn_project_worktree_links(&self) {
        let targets: Vec<(String, PathBuf, String)> = self
            .projects
            .iter()
            .filter(|project| self.project_wants_worktrees_link(project))
            .map(|project| {
                (
                    project.id.clone(),
                    PathBuf::from(&project.path),
                    project.name.clone(),
                )
            })
            .collect();
        self.spawn_worktree_link_worker(targets);
    }

    /// Ensure the link for one project, after an agent was created in it.
    pub fn spawn_project_worktree_link_for(&self, project_id: &str) {
        let targets: Vec<_> = self
            .projects
            .iter()
            .filter(|project| project.id == project_id)
            .filter(|project| self.project_wants_worktrees_link(project))
            .map(|project| {
                (
                    project.id.clone(),
                    PathBuf::from(&project.path),
                    project.name.clone(),
                )
            })
            .collect();
        self.spawn_worktree_link_worker(targets);
    }

    fn spawn_worktree_link_worker(&self, targets: Vec<(String, PathBuf, String)>) {
        if targets.is_empty() {
            return;
        }
        let worktrees_root = self.paths.worktrees_root.clone();
        // A plain named thread rather than a tracked worker: each call is
        // idempotent, posts no completion event and only logs, so overlapping
        // runs are harmless, and a panic in one only loses a log line.
        let spawned = std::thread::Builder::new()
            .name("dux-worktree-link".into())
            .spawn(move || {
                for (project_id, repo, name) in targets {
                    match crate::git::ensure_project_worktrees_link(&repo, &worktrees_root, &name) {
                        Ok(link) => crate::logger::debug(&format!(
                            "project {project_id}: worktree explorer link ready at {}",
                            link.display()
                        )),
                        Err(err) => crate::logger::warn(&format!(
                            "project {project_id}: could not expose agent worktrees in {}: {err:#}",
                            repo.display()
                        )),
                    }
                }
            });
        if let Err(err) = spawned {
            crate::logger::warn(&format!("could not start the worktree-link worker: {err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    #[test]
    fn shared_only_project_skips_link_but_isolated_agent_wants_it() {
        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let project = sample_project("p1", &repo.to_string_lossy());
        engine.projects.push(project.clone());
        assert!(
            !engine.project_wants_worktrees_link(&project),
            "no agents, no link"
        );

        let mut shared = sample_session("s1", "p1", "main");
        shared.shared_workspace = true;
        engine.sessions.push(shared);
        assert!(
            !engine.project_wants_worktrees_link(&project),
            "a shared-only project has no worktrees to show"
        );

        engine.sessions.push(sample_session("s2", "p1", "feat"));
        assert!(engine.project_wants_worktrees_link(&project));

        let mut missing = project.clone();
        missing.path_missing = true;
        assert!(!engine.project_wants_worktrees_link(&missing));
    }

    #[test]
    fn spawn_project_worktree_links_creates_the_link_off_thread() {
        let (mut engine, tmp) = test_engine();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(
            crate::git::test_support::git_command()
                .args(["init", "-q", "-b", "main"])
                .arg(&repo)
                .status()
                .unwrap()
                .success()
        );
        engine
            .projects
            .push(sample_project("p1", &repo.to_string_lossy()));
        engine.sessions.push(sample_session("s1", "p1", "feat"));

        engine.spawn_project_worktree_links();

        let link = repo.join(crate::git::PROJECT_WORKTREES_LINK_NAME);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::fs::symlink_metadata(&link).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "the worker never created the link"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
