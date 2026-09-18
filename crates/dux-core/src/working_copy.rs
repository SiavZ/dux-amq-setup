//! The sentences both surfaces say about a working copy that is gone from disk,
//! and about putting one back.
//!
//! A managed agent can lose the directory it lives in: a coding CLI that merges
//! its own branch and deletes it takes the worktree with it, from inside. dux
//! cannot prevent that (its own worktree manager already refuses to remove an
//! occupied directory), so what is owed is an honest verdict and a way out.
//!
//! The copy lives here, in one place, because the changes panel, the agent row,
//! the info panel, the confirmation dialog and the status line all say it, on
//! two surfaces, and a second wording is a second thing that can be wrong.

use std::path::Path;

use anyhow::{Result, anyhow};

use crate::home_path::shorten_home;

/// What recreating a working copy did to the agent's branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecreatedBranch {
    /// The branch was still in the repository and is checked out again. The
    /// agent's commits are exactly where it left them.
    CheckedOut,
    /// The branch was gone too, so it was created again from the project's
    /// source branch. It holds none of the commits it held before.
    RecreatedFrom(String),
}

/// Put a managed working copy back at the path it already had, from the branch
/// it already names.
///
/// The SAME path is the whole point: the coding CLIs key their conversation
/// history by directory path, so a working copy recreated anywhere else leaves
/// the agent unable to resume the conversation the user is trying to get back
/// to. What is not coming back is the content: this is a checkout, not a
/// restore, and whatever was uncommitted in the deleted directory is gone.
///
/// Refuses rather than overwrites when something already occupies the path.
/// dux is putting a directory back, and a directory that is already there is
/// somebody's, whether a working copy that reappeared or an unrelated folder.
pub fn recreate_working_copy(
    repo_path: &Path,
    worktree_path: &Path,
    branch_name: &str,
    source_branch: &str,
) -> Result<RecreatedBranch> {
    if worktree_path.exists() {
        return Err(anyhow!(
            "{} already exists, so dux left it alone. Move or remove it if you want dux to check \
             the branch out there again.",
            shorten_home(worktree_path)
        ));
    }
    // git still holds the registration of the directory the agent deleted, and
    // with it the branch as checked out, so both arms below fail without this.
    crate::git::prune_worktrees(repo_path)?;
    if crate::git::branch_exists(repo_path, branch_name).is_some() {
        crate::git::add_worktree_existing_branch_at(repo_path, worktree_path, branch_name)?;
        return Ok(RecreatedBranch::CheckedOut);
    }
    crate::git::add_worktree_new_branch_at(
        repo_path,
        worktree_path,
        branch_name,
        Some(source_branch),
    )?;
    Ok(RecreatedBranch::RecreatedFrom(source_branch.to_string()))
}

/// Why the changes region is quiet for a MANAGED agent whose working copy is
/// gone. Names the path, says what happened to a process still running there,
/// and says what has to happen before the agent can run again.
pub fn missing_working_copy_reason(worktree: &Path) -> String {
    format!(
        "The working copy at {} no longer exists on disk, so dux cannot show this agent's \
         changes. Anything still running in it keeps running in a directory that is gone, and \
         this agent cannot restart or resume here until the working copy is recreated.",
        shorten_home(worktree)
    )
}

/// The same fact for a STANDALONE agent, whose directory is the user's own.
/// dux never creates, moves or removes that folder, so the remedy is the user's
/// rather than a button.
pub fn missing_folder_reason(folder: &Path) -> String {
    format!(
        "The folder {} no longer exists on disk, so dux cannot show this agent's changes. \
         Anything still running in it keeps running in a directory that is gone. Restore the \
         folder, or delete this agent and create a new one pointing at the folder you want.",
        shorten_home(folder)
    )
}

/// The short label the agent row and the info panel show, where there is room
/// for a marker rather than a sentence.
pub const MISSING_WORKING_COPY_LABEL: &str = "working copy missing";

/// Why the changes region is quiet for one agent, or `None` when it is not.
///
/// The one place both surfaces and the wire ask, so a directory that is gone
/// reads the same everywhere. `managed` picks which remedy the missing-directory
/// sentence offers, because dux can put its own working copy back and never
/// touches the user's own folder.
pub fn quiet_reason(
    status: crate::git::FolderRepoStatus,
    directory: &Path,
    managed: bool,
) -> Option<String> {
    if status.changes_panel_works() {
        return None;
    }
    Some(match status {
        crate::git::FolderRepoStatus::Missing if managed => missing_working_copy_reason(directory),
        crate::git::FolderRepoStatus::Missing => missing_folder_reason(directory),
        other => other.quiet_reason().to_string(),
    })
}

/// What recreating the working copy costs, said plainly in the confirmation.
///
/// The uncommitted work in the old directory is gone with the directory: dux is
/// checking the branch out again, not restoring what was deleted. The
/// conversation may survive because the coding CLIs key their history by
/// directory path, which is the whole reason the working copy is recreated at
/// the SAME path rather than a fresh one.
/// Both branch arms are stated because asking git which one applies would run a
/// subprocess to open a dialog. The final status names the arm that actually
/// ran.
pub fn recreate_confirm_body(worktree: &Path, branch_name: &str, source_branch: &str) -> String {
    format!(
        "Recreate the working copy for this agent at {}?\n\nIf branch \"{branch_name}\" still \
         exists, dux checks it out there again. If it is gone too, dux creates it again from \
         \"{source_branch}\", and the commits that branch held are not coming back.\n\nAny code \
         changes that were in the old directory are gone either way: this puts the directory \
         back, not its contents. The conversation may resume, because the agent's CLI keys its \
         history by directory path and dux recreates the working copy at the same path.\n\nAnything \
         still running keeps working in the deleted directory; stop it and start the agent again \
         to work in the recreated copy.",
        shorten_home(worktree)
    )
}

/// The busy sentence while the recreate runs.
pub fn recreate_busy_message(agent_label: &str) -> String {
    format!("Recreating the working copy for agent \"{agent_label}\"...")
}

/// The success sentence, naming the path and what happened to the branch.
/// `source_branch` is `Some` only when the branch had to be created again.
pub fn recreate_success_message(
    agent_label: &str,
    worktree: &Path,
    branch_name: &str,
    source_branch: Option<&str>,
) -> String {
    let branch_outcome = match source_branch {
        Some(source) => format!(
            "branch \"{branch_name}\" was recreated from \"{source}\", so it holds none of the \
             commits it held before"
        ),
        None => format!("branch \"{branch_name}\" was checked out again"),
    };
    format!(
        "Recreated the working copy for agent \"{agent_label}\" at {}: {branch_outcome}. Its \
         tabs stay dormant; start one when you want the agent running there.",
        shorten_home(worktree)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_missing_reason_names_the_path_and_never_says_busy() {
        let reason = missing_working_copy_reason(Path::new("/tmp/worktrees/p/v0"));
        assert!(reason.contains("/tmp/worktrees/p/v0"), "{reason}");
        assert!(!reason.to_lowercase().contains("busy"), "{reason}");
        assert!(reason.contains("recreated"), "{reason}");
    }

    #[test]
    fn the_folder_reason_points_at_the_user_rather_than_a_button() {
        let reason = missing_folder_reason(Path::new("/tmp/mine"));
        assert!(reason.contains("/tmp/mine"), "{reason}");
        assert!(reason.contains("Restore the folder"), "{reason}");
        assert!(!reason.to_lowercase().contains("busy"), "{reason}");
    }

    #[test]
    fn the_quiet_reason_picks_the_remedy_from_the_kind() {
        use crate::git::FolderRepoStatus;
        let managed = quiet_reason(FolderRepoStatus::Missing, Path::new("/tmp/wt"), true)
            .expect("a missing working copy is quiet");
        assert!(managed.contains("recreated"), "{managed}");
        let folder = quiet_reason(FolderRepoStatus::Missing, Path::new("/tmp/mine"), false)
            .expect("a missing folder is quiet");
        assert!(folder.contains("Restore the folder"), "{folder}");
        assert_eq!(
            quiet_reason(FolderRepoStatus::WorkingRepo, Path::new("/tmp/wt"), true),
            None,
            "a working repository is not quiet"
        );
        let no_repo = quiet_reason(FolderRepoStatus::NoRepo, Path::new("/tmp/mine"), false)
            .expect("a plain folder is quiet");
        assert_eq!(no_repo, FolderRepoStatus::NoRepo.quiet_reason());
    }

    /// Pinned verbatim against `lib/recreateWorkingCopy.ts`, which carries the
    /// twin assertion, so a wording change fails on the side that changed.
    #[test]
    fn the_confirm_reads_the_same_on_both_surfaces() {
        assert_eq!(
            recreate_confirm_body(Path::new("/worktrees/repo/feat"), "feat", "main"),
            "Recreate the working copy for this agent at /worktrees/repo/feat?\n\n\
             If branch \"feat\" still exists, dux checks it out there again. If it is gone too, \
             dux creates it again from \"main\", and the commits that branch held are not coming \
             back.\n\nAny code changes that were in the old directory are gone either way: this \
             puts the directory back, not its contents. The conversation may resume, because the \
             agent's CLI keys its history by directory path and dux recreates the working copy at \
             the same path.\n\nAnything still running keeps working in the deleted directory; \
             stop it and start the agent again to work in the recreated copy."
        );
    }

    #[test]
    fn the_confirm_says_the_changes_are_gone_on_both_branch_outcomes() {
        let body = recreate_confirm_body(Path::new("/tmp/wt"), "feat", "main");
        assert!(body.contains("checks it out there again"), "{body}");
        assert!(body.contains("from \"main\""), "{body}");
        assert!(body.contains("not coming back"), "{body}");
        assert!(body.contains("are gone either way"), "{body}");
        assert!(body.contains("same path"), "{body}");
        assert!(body.contains("deleted directory"), "{body}");
    }

    /// The branch is still in the repository: the working copy comes back at the
    /// SAME path, which is what lets the CLI find its conversation again.
    #[test]
    fn a_surviving_branch_is_checked_out_again_at_the_same_path() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        let outcome = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect("the branch is still there");
        assert_eq!(outcome, RecreatedBranch::CheckedOut);
        assert!(worktree.join("seed.txt").exists(), "the checkout landed");
        assert!(
            worktree.join("on-the-branch.txt").exists(),
            "and it is the agent's branch, with its commits"
        );
    }

    /// The agent deleted its branch as well. The working copy still comes back,
    /// from the project's source branch, and the answer says so rather than
    /// implying the commits survived.
    #[test]
    fn a_deleted_branch_is_minted_again_from_the_source_branch() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        crate::git::prune_worktrees(repo.path()).expect("prune");
        run_git(repo.path(), &["branch", "-D", "feat"]);

        let outcome = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect("the source branch is there");
        assert_eq!(outcome, RecreatedBranch::RecreatedFrom("main".to_string()));
        assert!(worktree.join("seed.txt").exists());
        assert!(
            !worktree.join("on-the-branch.txt").exists(),
            "a branch minted from main holds none of the agent's commits"
        );
    }

    /// Something already occupies the path. dux is putting a directory back, and
    /// a directory that is already there is somebody's.
    #[test]
    fn an_occupied_path_is_refused_rather_than_overwritten() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        std::fs::create_dir_all(&worktree).expect("occupy the path");
        std::fs::write(worktree.join("mine.txt"), "do not touch").expect("write");

        let err = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect_err("an occupied path is refused");
        let message = format!("{err:#}");
        assert!(message.contains("already exists"), "{message}");
        assert_eq!(
            std::fs::read_to_string(worktree.join("mine.txt")).expect("still there"),
            "do not touch"
        );
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(repo)
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A repository with a worktree whose directory has been deleted from under
    /// it, exactly as an agent that merges its branch and removes its own
    /// worktree leaves things.
    fn repo_with_a_deleted_worktree() -> (tempfile::TempDir, std::path::PathBuf) {
        let repo = tempfile::tempdir().expect("repo");
        run_git(repo.path(), &["init", "-q", "-b", "main", "."]);
        run_git(repo.path(), &["config", "user.email", "dux@example.com"]);
        run_git(repo.path(), &["config", "user.name", "dux"]);
        std::fs::write(repo.path().join("seed.txt"), "seed").expect("seed");
        run_git(repo.path(), &["add", "seed.txt"]);
        run_git(repo.path(), &["commit", "-qm", "seed"]);

        let worktree = repo.path().join("wt").join("v0");
        std::fs::create_dir_all(worktree.parent().expect("parent")).expect("worktrees root");
        crate::git::add_worktree_new_branch_at(repo.path(), &worktree, "feat", Some("main"))
            .expect("the agent's working copy");
        std::fs::write(worktree.join("on-the-branch.txt"), "work").expect("work");
        run_git(&worktree, &["add", "on-the-branch.txt"]);
        run_git(&worktree, &["commit", "-qm", "work"]);
        std::fs::remove_dir_all(&worktree).expect("the agent deletes its own working copy");
        (repo, worktree)
    }

    #[test]
    fn the_success_message_names_the_path_and_the_branch_outcome() {
        let checked_out = recreate_success_message("v0", Path::new("/tmp/wt"), "feat", None);
        assert!(checked_out.contains("/tmp/wt"), "{checked_out}");
        assert!(checked_out.contains("checked out again"), "{checked_out}");
        assert!(checked_out.contains("dormant"), "{checked_out}");
        let minted = recreate_success_message("v0", Path::new("/tmp/wt"), "feat", Some("main"));
        assert!(minted.contains("recreated from \"main\""), "{minted}");
    }
}
