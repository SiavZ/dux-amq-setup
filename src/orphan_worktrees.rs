//! Fail-closed inventory and guarded removal for opt-in orphan worktree cleanup.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::config::{self, DuxPaths};
use crate::git;
use crate::storage::{SessionStore, load_store_id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OrphanWorktreeCandidate {
    pub(crate) project_path: PathBuf,
    pub(crate) worktree_path: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) dirty: bool,
}

struct Inventory {
    candidates: Vec<OrphanWorktreeCandidate>,
    registered_projects: Vec<PathBuf>,
}

/// Build the complete candidate list before the UI offers any removal.
pub(crate) fn inventory(paths: &DuxPaths) -> Result<Vec<OrphanWorktreeCandidate>> {
    Ok(load_inventory(paths)?.candidates)
}

/// Revalidate the complete inventory, then remove exactly one still-eligible
/// candidate through the central whole-workspace guard.
pub(crate) fn remove(paths: &DuxPaths, worktree_path: &Path, delete_branch: bool) -> Result<()> {
    let inventory = load_inventory(paths)?;
    let target = git::resolve_whole_workspace_path(worktree_path)?;
    let candidate = inventory
        .candidates
        .iter()
        .find(|candidate| candidate.worktree_path == target)
        .ok_or_else(|| {
            anyhow!(
                "worktree is no longer an eligible orphan: {}",
                crate::sanitize::for_terminal(&target.display().to_string())
            )
        })?;
    if delete_branch && candidate.branch.is_none() {
        bail!("a detached worktree has no branch to delete");
    }
    git::remove_worktree(
        &candidate.project_path,
        &candidate.worktree_path,
        candidate.branch.as_deref().unwrap_or(""),
        delete_branch,
        &inventory.registered_projects,
    )?;
    Ok(())
}

fn load_inventory(paths: &DuxPaths) -> Result<Inventory> {
    let config_path = crate::sanitize::for_terminal(&paths.config_path.display().to_string());
    let database_path =
        crate::sanitize::for_terminal(&paths.sessions_db_path.display().to_string());
    let config = config::load_config_read_only(paths).with_context(|| {
        format!("orphan cleanup aborted before removal: repair {config_path}, then retry")
    })?;
    let registered_projects = config::registered_project_paths(&config).with_context(|| {
        format!(
            "orphan cleanup aborted before removal: repair the project inventory in {config_path}, then retry"
        )
    })?;
    load_store_id(&paths.root).with_context(|| {
        format!(
            "orphan cleanup aborted before removal: restore {}, then retry",
            crate::sanitize::for_terminal(&paths.root.join("store-id").display().to_string())
        )
    })?;
    let sessions = if paths.sessions_db_path.exists() {
        SessionStore::open_read_only(&paths.sessions_db_path)
            .with_context(|| {
                format!(
                    "orphan cleanup aborted before removal: repair {database_path} or restore {database_path}.bak, then retry"
                )
            })?
            .load_sessions_including_deleted()
            .with_context(|| {
                format!(
                    "orphan cleanup aborted before removal: repair the named row in {database_path} or restore {database_path}.bak, then retry"
                )
            })?
    } else {
        Vec::new()
    };
    let session_paths = sessions
        .iter()
        .map(|session| {
            git::resolve_whole_workspace_path(Path::new(&session.worktree_path)).with_context(
                || {
                    format!(
                        "failed to resolve worktree for session {}",
                        crate::sanitize::for_terminal(&session.id)
                    )
                },
            )
        })
        .collect::<Result<HashSet<_>>>()?;

    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for project_path in &registered_projects {
        let project_path = git::resolve_whole_workspace_path(project_path)?;
        let worktrees = git::registered_worktrees(&project_path).with_context(|| {
            format!(
                "orphan cleanup aborted before removal: could not load the Git worktree inventory for {}",
                crate::sanitize::for_terminal(&project_path.display().to_string())
            )
        })?;
        for worktree in worktrees {
            if worktree.is_main {
                continue;
            }
            let worktree_path = git::resolve_whole_workspace_path(&worktree.path)?;
            if !git::whole_workspace_target_is_within(&paths.worktrees_root, &worktree_path)?
                || session_paths.contains(&worktree_path)
                || !seen.insert(worktree_path.clone())
            {
                continue;
            }
            let dirty = git::worktree_is_dirty(&worktree_path).with_context(|| {
                format!(
                    "orphan cleanup aborted before removal: could not inspect {}",
                    crate::sanitize::for_terminal(&worktree_path.display().to_string())
                )
            })?;
            candidates.push(OrphanWorktreeCandidate {
                project_path: project_path.clone(),
                worktree_path,
                branch: worktree.branch,
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

    use crate::config::{Config, ProjectConfig, WorkspaceConfig, WorkspaceMode};
    use crate::model::{AgentSession, ProviderKind, SessionSettings, SessionState};
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
        let mut config = Config {
            workspace: Some(WorkspaceConfig {
                default_mode: WorkspaceMode::Worktree,
                auto_resume_shared: false,
            }),
            ..Config::default()
        };
        config.projects = project_paths
            .iter()
            .enumerate()
            .map(|(index, path)| ProjectConfig {
                id: format!("project-{index}"),
                path: path.to_string_lossy().into_owned(),
                name: Some(format!("project-{index}")),
                default_provider: None,
                commit_prompt: None,
                workspace_mode: Some(WorkspaceMode::Worktree),
            })
            .collect();
        let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
        fs::write(
            &paths.config_path,
            crate::config::render_config_with(&config, &bindings),
        )
        .unwrap();
    }

    fn session_for(path: &Path) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: "retained-row".to_string(),
            project_id: "project-0".to_string(),
            project_path: None,
            provider: ProviderKind::from_str("claude"),
            source_branch: "main".to_string(),
            branch_name: "orphan".to_string(),
            worktree_path: path.to_string_lossy().into_owned(),
            agent_handle: "retained-row".to_string(),
            shared_workspace: false,
            deleted_at: None,
            title: None,
            started_providers: Vec::new(),
            provider_session_ids: Default::default(),
            state: SessionState::Created { created_at: now },
            settings: SessionSettings::default(),
            created_at: now,
            updated_at: now,
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
        // Proves the `is_main` exclusion is load-bearing: when a project's own
        // repo sits INSIDE worktrees_root, the containment check no longer
        // filters its main worktree, so `is_main` is the only guard against
        // offering the live main checkout as an orphan. (guard_whole_workspace
        // _removal independently protects it too, but the inventory must never
        // even list it.)
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
        write_config(&paths, &[&repo, &nested_project]);

        let error = remove(&paths, &managed, false).unwrap_err().to_string();
        assert!(error.contains("overlaps registered project"));
        assert!(managed.exists());
    }
}
