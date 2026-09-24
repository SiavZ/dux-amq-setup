//! Fail-closed inventory and guarded removal for the opt-in orphan-worktree
//! cleaner (fork 5c9edb78).
//!
//! An orphan is a git-registered, non-main worktree of a registered project
//! that sits strictly inside dux's managed worktrees root and that NO session
//! row names, tombstones included. Everything else is excluded: the project
//! checkout itself, a worktree outside the managed root, an arbitrary directory
//! under the root that git does not know about, and a worktree a live or
//! soft-deleted agent still owns (a tombstone keeps its worktree so a later
//! hard purge can still find it).
//!
//! This differs from upstream's per-project worktree manager
//! ([`crate::worktree_manager`]) in three ways that are the reason it exists:
//! it looks across every project at once, it counts tombstones as owners, and
//! it fails closed. If any part of the inventory cannot be loaded (the config,
//! the project list, the store identity, the session rows, a project's git
//! worktree list, a candidate's dirty state), NOTHING is offered and nothing is
//! removed. Removal re-runs the complete inventory first, so a worktree an agent
//! claimed between listing and confirming is no longer eligible, and every
//! removal re-enters the central protected-workspace guard.
//!
//! The branch is preserved by default. It is deleted only when the caller asks
//! for it explicitly, per item.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::DuxPaths;
use crate::git;
use crate::sanitize::for_terminal;
use crate::storage::{SessionStore, load_store_id};

/// One worktree the cleaner may offer for removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanWorktreeCandidate {
    /// The owning repository, as registered.
    pub project_path: PathBuf,
    /// The worktree, resolved symlink-aware.
    pub worktree_path: PathBuf,
    /// The checked-out branch, `None` when detached.
    pub branch: Option<String>,
    /// Uncommitted or untracked work: removal is forced and deletes it.
    pub dirty: bool,
}

struct Inventory {
    candidates: Vec<OrphanWorktreeCandidate>,
    registered_projects: Vec<PathBuf>,
}

/// Build the complete candidate list. Fails closed; see the module doc.
pub fn inventory(paths: &DuxPaths) -> Result<Vec<OrphanWorktreeCandidate>> {
    Ok(load_inventory(paths)?.candidates)
}

/// Revalidate the complete inventory, then remove exactly one still-eligible
/// candidate through the protected-workspace guard. `delete_branch` also
/// force-deletes its branch; the default is to keep it.
pub fn remove(paths: &DuxPaths, worktree_path: &Path, delete_branch: bool) -> Result<()> {
    let inventory = load_inventory(paths)?;
    let target = resolve(worktree_path)?;
    let candidate = inventory
        .candidates
        .iter()
        .find(|candidate| candidate.worktree_path == target)
        .ok_or_else(|| {
            anyhow!(
                "worktree is no longer an eligible orphan: {}",
                for_terminal(&target.display().to_string())
            )
        })?;
    match (delete_branch, candidate.branch.as_deref()) {
        (true, None) => bail!("a detached worktree has no branch to delete"),
        (true, Some(branch)) => {
            git::remove_worktree(
                &candidate.project_path,
                &candidate.worktree_path,
                branch,
                None,
                &inventory.registered_projects,
            )?;
        }
        (false, _) => git::remove_worktree_keep_branch(
            &candidate.project_path,
            &candidate.worktree_path,
            &inventory.registered_projects,
        )?,
    }
    // `git worktree remove --force` can report success yet leave the directory
    // behind (a nested untracked repository, a permissions quirk). Say so
    // rather than claim a removal that did not happen.
    if candidate.worktree_path.exists() {
        bail!(
            "git reported success but {} is still on disk",
            for_terminal(&candidate.worktree_path.display().to_string())
        );
    }
    Ok(())
}

/// Symlink-aware resolution shared with the protected-workspace guard, so an
/// alias cannot make one worktree look like two.
fn resolve(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!(
            "orphan worktree path must be absolute: {}",
            for_terminal(&path.display().to_string())
        );
    }
    Ok(crate::project_browser::canonical_or_original(path))
}

fn load_inventory(paths: &DuxPaths) -> Result<Inventory> {
    let config_path = for_terminal(&paths.config_path.display().to_string());
    let database_path = for_terminal(&paths.sessions_db_path.display().to_string());
    let abort = "orphan cleanup aborted before removal";

    let config = crate::purge::load_config_strict(paths)
        .with_context(|| format!("{abort}: repair {config_path}, then retry"))?;
    load_store_id(&paths.root).with_context(|| {
        format!(
            "{abort}: restore {}, then retry",
            for_terminal(&paths.root.join("store-id").display().to_string())
        )
    })?;
    // An absent database has no rows; anything else must open and load whole.
    let (store, sessions) = if paths.sessions_db_path.exists() {
        let store = SessionStore::open(&paths.sessions_db_path).with_context(|| {
            format!("{abort}: repair {database_path} or restore {database_path}.bak, then retry")
        })?;
        let sessions = store.load_sessions_including_deleted().with_context(|| {
            format!(
                "{abort}: repair the named row in {database_path} or restore {database_path}.bak, then retry"
            )
        })?;
        (Some(store), sessions)
    } else {
        (None, Vec::new())
    };
    let registered_projects = match &store {
        Some(store) => crate::purge::protected_project_paths(&config, store, &sessions),
        None => git::registered_project_paths(config.projects.iter().map(|p| p.path.as_str())),
    }
    .with_context(|| {
        format!("{abort}: repair the project inventory in {config_path}, then retry")
    })?;

    let session_paths = sessions
        .iter()
        .map(|session| {
            resolve(Path::new(session.directory())).with_context(|| {
                format!(
                    "{abort}: failed to resolve the directory of session {}",
                    for_terminal(&session.id)
                )
            })
        })
        .collect::<Result<HashSet<_>>>()?;
    let project_set = registered_projects
        .iter()
        .map(|project| resolve(project))
        .collect::<Result<HashSet<_>>>()?;

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    let mut scanned = HashSet::new();
    for project_path in &registered_projects {
        let project_path = resolve(project_path)?;
        if !scanned.insert(project_path.clone()) {
            continue;
        }
        let worktrees = git::list_worktrees(&project_path).with_context(|| {
            format!(
                "{abort}: could not load the Git worktree inventory for {}",
                for_terminal(&project_path.display().to_string())
            )
        })?;
        // `git worktree list` always reports the main worktree first. It is
        // never a candidate, even when the project itself lives inside the
        // managed root; neither is any registered project checkout.
        for worktree in worktrees.into_iter().skip(1) {
            let worktree_path = resolve(&worktree.path)?;
            if project_set.contains(&worktree_path)
                || !git::whole_workspace_target_is_within(&paths.worktrees_root, &worktree_path)?
                || session_paths.contains(&worktree_path)
                || !seen.insert(worktree_path.clone())
            {
                continue;
            }
            let dirty = git::worktree_is_dirty(&worktree_path).with_context(|| {
                format!(
                    "{abort}: could not inspect {}",
                    for_terminal(&worktree_path.display().to_string())
                )
            })?;
            candidates.push(OrphanWorktreeCandidate {
                project_path: project_path.clone(),
                worktree_path,
                branch: if worktree.detached {
                    None
                } else {
                    worktree.branch_name
                },
                dirty,
            });
        }
    }
    candidates.sort_by(|left, right| left.worktree_path.cmp(&right.worktree_path));
    Ok(Inventory {
        candidates,
        registered_projects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    use chrono::Utc;
    use tempfile::tempdir;

    use crate::config::{Config, ProjectConfig};
    use crate::model::{
        AgentSession, AgentWorkspace, BranchProvenance, ManagedWorkspace, ProviderKind,
        SessionStatus,
    };
    use crate::storage::load_or_create_store_id;

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        run_git(path, &["init", "-b", "main"]);
        run_git(path, &["config", "user.name", "test"]);
        run_git(path, &["config", "user.email", "test@example.com"]);
        run_git(path, &["commit", "--allow-empty", "-m", "initial"]);
    }

    fn test_paths(root: PathBuf) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        }
    }

    fn write_config(paths: &DuxPaths, project_paths: &[&Path]) {
        let config = Config {
            projects: project_paths
                .iter()
                .enumerate()
                .map(|(index, path)| ProjectConfig {
                    id: format!("project-{index}"),
                    path: path.to_string_lossy().into_owned(),
                    name: Some(format!("project-{index}")),
                    default_provider: None,
                    leading_branch: None,
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: Default::default(),
                })
                .collect(),
            ..Config::default()
        };
        fs::write(&paths.config_path, toml::to_string(&config).unwrap()).unwrap();
    }

    fn session_for(path: &Path) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: "retained-row".to_string(),
            agent_handle: "retained-row".to_string(),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: "retained-row-slot".to_string(),
            provider: ProviderKind::new("claude"),
            workspace: AgentWorkspace::Managed(ManagedWorkspace {
                project_id: "project-0".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: "orphan".to_string(),
                initial_branch: "orphan".to_string(),
                branch_provenance: BranchProvenance::CreatedByDux,
                worktree_path: path.to_string_lossy().into_owned(),
            }),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    }

    fn setup_inventory() -> (tempfile::TempDir, DuxPaths, PathBuf, PathBuf, PathBuf) {
        let temp = tempdir().unwrap();
        let repo = temp.path().join("repo");
        let paths = test_paths(temp.path().join("dux"));
        fs::create_dir_all(&paths.worktrees_root).unwrap();
        init_repo(&repo);
        let managed = paths.worktrees_root.join("orphan");
        run_git(
            &repo,
            &["worktree", "add", "-b", "orphan", managed.to_str().unwrap()],
        );
        let outside = temp.path().join("user-worktree");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "outside",
                outside.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(paths.worktrees_root.join("ordinary-user-dir")).unwrap();
        write_config(&paths, &[&repo]);
        load_or_create_store_id(&paths.root).unwrap();
        SessionStore::open(&paths.sessions_db_path).unwrap();
        (temp, paths, repo, managed, outside)
    }

    #[test]
    fn inventory_excludes_a_project_main_worktree_even_inside_worktrees_root() {
        // The main-worktree exclusion is load-bearing: when a project's own
        // repo sits INSIDE worktrees_root, containment no longer filters it, so
        // skipping git's first (main) entry is the only thing keeping the live
        // checkout off the list. The guard would refuse the removal anyway,
        // but the inventory must never even offer it.
        let temp = tempdir().unwrap();
        let paths = test_paths(temp.path().join("dux"));
        fs::create_dir_all(&paths.worktrees_root).unwrap();
        let repo = paths.worktrees_root.join("in-root-repo");
        init_repo(&repo);
        let managed = paths.worktrees_root.join("orphan");
        run_git(
            &repo,
            &["worktree", "add", "-b", "orphan", managed.to_str().unwrap()],
        );
        write_config(&paths, &[&repo]);
        load_or_create_store_id(&paths.root).unwrap();
        SessionStore::open(&paths.sessions_db_path).unwrap();

        let candidates = inventory(&paths).unwrap();
        assert_eq!(
            candidates.len(),
            1,
            "only the linked orphan, never the main checkout"
        );
        assert_eq!(candidates[0].worktree_path, managed.canonicalize().unwrap());
        assert!(
            !candidates
                .iter()
                .any(|c| c.worktree_path == repo.canonicalize().unwrap()),
            "the project's main worktree must be excluded even inside worktrees_root"
        );
    }

    #[test]
    fn inventory_admits_only_registered_root_orphans_and_preserves_branch_by_default() {
        let (_temp, paths, repo, managed, outside) = setup_inventory();

        let candidates = inventory(&paths).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].worktree_path, managed.canonicalize().unwrap());
        assert_eq!(candidates[0].branch.as_deref(), Some("orphan"));
        assert!(!candidates[0].dirty);
        assert!(outside.exists(), "outside registered worktree is excluded");
        assert!(
            paths.worktrees_root.join("ordinary-user-dir").exists(),
            "an arbitrary root directory is excluded"
        );
        assert!(repo.exists(), "the main worktree is excluded");

        fs::write(managed.join("untracked.txt"), "keep me").unwrap();
        assert!(inventory(&paths).unwrap()[0].dirty);

        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store.upsert_session(&session_for(&managed)).unwrap();
        assert!(inventory(&paths).unwrap().is_empty());
        store.soft_delete_session("retained-row").unwrap();
        assert!(
            inventory(&paths).unwrap().is_empty(),
            "a tombstone still owns its worktree"
        );
        store.delete_session("retained-row").unwrap();
        drop(store);

        remove(&paths, &managed, false).unwrap();
        assert!(!managed.exists());
        let branch = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["show-ref", "--verify", "refs/heads/orphan"])
            .output()
            .unwrap();
        assert!(branch.status.success(), "branch is preserved by default");
    }

    #[test]
    fn cleanup_aborts_before_removal_when_any_inventory_is_incomplete() {
        let (_temp, paths, repo, managed, _outside) = setup_inventory();

        fs::write(&paths.config_path, "not = [valid").unwrap();
        assert!(inventory(&paths).is_err());
        assert!(remove(&paths, &managed, false).is_err());
        assert!(managed.exists());

        write_config(&paths, &[&repo]);
        fs::write(&paths.sessions_db_path, "not sqlite").unwrap();
        assert!(inventory(&paths).is_err());
        assert!(managed.exists());

        fs::remove_file(&paths.sessions_db_path).unwrap();
        SessionStore::open(&paths.sessions_db_path).unwrap();
        fs::remove_file(paths.root.join("store-id")).unwrap();
        assert!(inventory(&paths).is_err());
        assert!(managed.exists());

        load_or_create_store_id(&paths.root).unwrap();
        let non_repo = paths.root.join("not-a-repository");
        fs::create_dir_all(&non_repo).unwrap();
        write_config(&paths, &[&repo, &non_repo]);
        assert!(inventory(&paths).is_err());
        assert!(managed.exists());
    }

    #[test]
    fn removal_reenters_whole_workspace_guard() {
        let (_temp, paths, repo, managed, _outside) = setup_inventory();
        let nested_project = managed.join("nested-project");
        fs::create_dir_all(&nested_project).unwrap();
        init_repo(&nested_project);
        write_config(&paths, &[&repo, &nested_project]);

        let error = format!("{:#}", remove(&paths, &managed, false).unwrap_err());
        assert!(error.contains("overlaps the registered project"), "{error}");
        assert!(managed.exists());
    }

    /// Opting into branch deletion deletes exactly the orphan's branch; a
    /// detached orphan refuses the request instead of guessing a branch.
    #[test]
    fn removal_deletes_branch_only_when_asked_and_refuses_for_detached() {
        let (_temp, paths, repo, managed, _outside) = setup_inventory();
        remove(&paths, &managed, true).unwrap();
        assert!(!managed.exists());
        let branch = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["show-ref", "--verify", "--quiet", "refs/heads/orphan"])
            .status()
            .unwrap();
        assert!(!branch.success(), "the opted-in branch is deleted");

        let detached = paths.worktrees_root.join("detached");
        run_git(
            &repo,
            &["worktree", "add", "--detach", detached.to_str().unwrap()],
        );
        let error = remove(&paths, &detached, true).unwrap_err().to_string();
        assert!(error.contains("detached"), "{error}");
        assert!(detached.exists());
    }
}
