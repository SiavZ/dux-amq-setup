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
    /// The branch was gone locally but still on the remote, so it was created
    /// again from the remote-tracking ref. It holds everything that had been
    /// pushed and nothing after that.
    RecreatedFromRemote(String),
    /// The branch was gone everywhere, so it was created again from the
    /// project's source branch. It holds none of the commits it held before.
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
    match crate::git::directory_presence(worktree_path) {
        crate::git::DirectoryPresence::Missing => {}
        crate::git::DirectoryPresence::Present => {
            return Err(anyhow!(
                "{} already exists, so dux left it alone. Move or remove it if you want dux to \
                 check the branch out there again.",
                shorten_home(worktree_path)
            ));
        }
        // The same fail-closed rule the verdict uses: a stat that failed for a
        // reason other than "nothing is there" means dux does not know whether
        // the old working copy is still sitting at this path, and checking a
        // branch out over it is not a guess dux may take.
        crate::git::DirectoryPresence::Indeterminate => {
            return Err(anyhow!(
                "dux could not read {}, so it does not know whether the old working copy is still \
                 there and left it alone. Check that the path is readable and that whatever it \
                 lives on is mounted, then try again.",
                shorten_home(worktree_path)
            ));
        }
    }
    // git still holds the registration of the directory the agent deleted, and
    // with it the branch as checked out, so both arms below fail without this.
    // Only THIS path's registration: a repository-wide prune takes any sibling
    // agent whose directory is unreachable at that instant with it.
    crate::git::forget_missing_worktree_registration(repo_path, worktree_path)?;
    // LOCAL only. `branch_exists` also answers Some for a branch that survives
    // only as a remote-tracking ref, and checking that out would put the agent
    // on a detached HEAD rather than on its branch. A branch that is gone
    // locally but still on the remote gets its own arm, because its pushed
    // commits are recoverable and starting again from the source branch would
    // throw them away without saying so.
    match crate::git::branch_exists(repo_path, branch_name) {
        Some(crate::git::BranchLocation::Local) => {
            crate::git::add_worktree_existing_branch_at(repo_path, worktree_path, branch_name)?;
            Ok(RecreatedBranch::CheckedOut)
        }
        Some(crate::git::BranchLocation::Remote) => {
            let remote_ref = format!("refs/remotes/origin/{branch_name}");
            crate::git::add_worktree_new_branch_at(
                repo_path,
                worktree_path,
                branch_name,
                Some(&remote_ref),
            )?;
            Ok(RecreatedBranch::RecreatedFromRemote(format!(
                "origin/{branch_name}"
            )))
        }
        None => {
            crate::git::add_worktree_new_branch_at(
                repo_path,
                worktree_path,
                branch_name,
                Some(source_branch),
            )?;
            Ok(RecreatedBranch::RecreatedFrom(source_branch.to_string()))
        }
    }
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

/// Why the changes region is quiet for a managed agent whose working copy dux
/// could not stat at all.
///
/// Deliberately hedged, and deliberately not the missing sentence: an
/// unreadable parent, a stale handle and a mount that is down all fail the same
/// stat a deletion does, and telling the user their working copy was deleted
/// when it is sitting safely on a mount that is merely down would have them
/// recreate a branch that never needed recreating.
pub fn unreachable_working_copy_reason(worktree: &Path) -> String {
    format!(
        "dux could not read the working copy at {}, so it cannot say whether this agent has \
         changes. Check that the path is readable and that whatever it lives on is mounted, then \
         reopen this panel.",
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
        // A managed working copy dux could not stat is not "dux could not
        // consult git about this folder": nothing was asked of git at all.
        crate::git::FolderRepoStatus::Indeterminate if managed => {
            unreachable_working_copy_reason(directory)
        }
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
/// All three branch arms are stated because asking git which one applies would
/// run a subprocess to open a dialog. The final status names the arm that
/// actually ran.
pub fn recreate_confirm_body(worktree: &Path, branch_name: &str, source_branch: &str) -> String {
    format!(
        "Recreate the working copy for this agent at {}?\n\nIf branch \"{branch_name}\" still \
         exists locally, dux checks it out there again. If it is gone locally but still on the \
         remote, dux creates it again from \"origin/{branch_name}\", holding everything that had \
         been pushed. If it is gone everywhere, dux creates it again from \"{source_branch}\", and \
         the commits that branch held are not coming back.\n\nAny code \
         changes that were in the old directory are gone either way: this puts the directory \
         back, not its contents. The conversation may resume, because the agent's CLI keys its \
         history by directory path and dux recreates the working copy at the same path.\n\nIts tabs \
         are dormant and stay that way, because dux refuses this while the agent is running. A \
         terminal still open in the old directory keeps working in a directory that is gone; close \
         it and open one in the recreated copy.",
        shorten_home(worktree)
    )
}

/// The busy sentence while the recreate runs.
pub fn recreate_busy_message(agent_label: &str) -> String {
    format!("Recreating the working copy for agent \"{agent_label}\"...")
}

/// The success sentence, naming the path and what actually happened to the
/// branch.
///
/// Takes the outcome rather than an optional start point so the three arms
/// cannot collapse into two: a branch rebuilt from the remote holds the agent's
/// own pushed commits, and a branch rebuilt from the source branch holds none
/// of them, which is not a difference to leave to a caller's `Option`.
pub fn recreate_success_message(
    agent_label: &str,
    worktree: &Path,
    branch_name: &str,
    outcome: &RecreatedBranch,
) -> String {
    let branch_outcome = match outcome {
        RecreatedBranch::RecreatedFrom(source) => format!(
            "branch \"{branch_name}\" was recreated from \"{source}\", so it holds none of the \
             commits it held before"
        ),
        RecreatedBranch::RecreatedFromRemote(remote) => format!(
            "branch \"{branch_name}\" was gone locally and was recreated from \"{remote}\", so it \
             holds what had been pushed there and nothing committed after that"
        ),
        RecreatedBranch::CheckedOut => format!("branch \"{branch_name}\" was checked out again"),
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
             If branch \"feat\" still exists locally, dux checks it out there again. If it is \
             gone locally but still on the remote, dux creates it again from \"origin/feat\", \
             holding everything that had been pushed. If it is gone everywhere, dux creates it \
             again from \"main\", and the commits that branch held are not coming \
             back.\n\nAny code changes that were in the old directory are gone either way: this \
             puts the directory back, not its contents. The conversation may resume, because the \
             agent's CLI keys its history by directory path and dux recreates the working copy at \
             the same path.\n\nIts tabs are dormant and stay that way, because dux refuses this \
             while the agent is running. A terminal still open in the old directory keeps working \
             in a directory that is gone; close it and open one in the recreated copy."
        );
    }

    #[test]
    fn the_confirm_says_the_changes_are_gone_on_both_branch_outcomes() {
        let body = recreate_confirm_body(Path::new("/tmp/wt"), "feat", "main");
        assert!(body.contains("checks it out there again"), "{body}");
        assert!(body.contains("from \"origin/feat\""), "{body}");
        assert!(body.contains("from \"main\""), "{body}");
        assert!(body.contains("not coming back"), "{body}");
        assert!(body.contains("are gone either way"), "{body}");
        assert!(body.contains("same path"), "{body}");
        assert!(
            body.contains("refuses this while the agent is running"),
            "{body}"
        );
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
        crate::git::forget_missing_worktree_registration(repo.path(), &worktree)
            .expect("forget the registration so the branch can be deleted");
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

    /// The regression that made the prune targeted. Measured on git 2.55:
    /// `git worktree prune` removes EVERY registration whose directory is
    /// unreachable at that instant, so recreating one agent's working copy while
    /// a sibling's directory was merely renamed away severed the sibling
    /// permanently, and moving its directory back left `git status` in it
    /// answering "not a git repository".
    #[test]
    fn recreating_one_working_copy_leaves_a_sibling_on_an_unreachable_path_alone() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        let sibling = repo.path().join("wt").join("v1");
        crate::git::add_worktree_new_branch_at(repo.path(), &sibling, "other", Some("main"))
            .expect("the sibling agent's working copy");
        let stashed = repo.path().join("wt").join("v1-unreachable");
        std::fs::rename(&sibling, &stashed).expect("the sibling's mount goes away");

        recreate_working_copy(repo.path(), &worktree, "feat", "main").expect("the recreate runs");

        std::fs::rename(&stashed, &sibling).expect("the sibling's mount comes back");
        let listed = crate::git::test_support::git_command()
            .args(["-C", &repo.path().to_string_lossy(), "worktree", "list"])
            .output()
            .expect("git runs");
        let listing = String::from_utf8_lossy(&listed.stdout);
        assert!(
            listing.contains("[other]"),
            "the sibling's registration survived: {listing}"
        );
        let status = crate::git::test_support::git_command()
            .args(["-C", &sibling.to_string_lossy(), "status", "--porcelain=v1"])
            .output()
            .expect("git runs");
        assert!(
            status.status.success(),
            "and its directory still works: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    /// A branch that survives only as a remote-tracking ref is NOT checked out:
    /// `branch_exists` answers Some for one, and taking the checkout arm would
    /// leave the agent on a detached HEAD rather than on its branch. It gets its
    /// own arm, because the commits it pushed are recoverable and starting again
    /// from the source branch would throw them away silently.
    #[test]
    fn a_branch_that_survives_only_on_the_remote_is_recreated_from_the_remote() {
        let (repo, worktree) = repo_with_a_deleted_worktree();
        // Give the repository an origin pointing at itself, so the agent's
        // branch has somewhere to have been pushed, then delete it locally.
        run_git(
            repo.path(),
            &["remote", "add", "origin", &repo.path().to_string_lossy()],
        );
        run_git(repo.path(), &["fetch", "-q", "origin"]);
        crate::git::forget_missing_worktree_registration(repo.path(), &worktree)
            .expect("forget the registration");
        run_git(repo.path(), &["branch", "-D", "--", "feat"]);
        assert_eq!(
            crate::git::branch_exists(repo.path(), "feat"),
            Some(crate::git::BranchLocation::Remote),
            "precondition: the branch is gone locally and still on the remote"
        );

        let outcome = recreate_working_copy(repo.path(), &worktree, "feat", "main")
            .expect("the remote-tracking ref is there");
        assert_eq!(
            outcome,
            RecreatedBranch::RecreatedFromRemote("origin/feat".to_string())
        );
        assert!(
            worktree.join("on-the-branch.txt").exists(),
            "the pushed commits came back"
        );
        let head = crate::git::test_support::git_command()
            .args([
                "-C",
                &worktree.to_string_lossy(),
                "symbolic-ref",
                "--quiet",
                "--short",
                "HEAD",
            ])
            .output()
            .expect("git runs");
        assert_eq!(
            String::from_utf8_lossy(&head.stdout).trim(),
            "feat",
            "and the agent is on its branch, not a detached HEAD"
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

    /// A stat dux could not get an answer to must not be treated as a deletion:
    /// checking a branch out over a working copy that is merely unreachable is
    /// exactly the loss this refusal exists to prevent.
    #[test]
    fn an_unreadable_path_is_refused_rather_than_checked_out_over() {
        use std::os::unix::fs::PermissionsExt;

        let (repo, _worktree) = repo_with_a_deleted_worktree();
        let locked = repo.path().join("locked");
        std::fs::create_dir_all(locked.join("inner")).expect("inner");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("lock");
        let result = recreate_working_copy(repo.path(), &locked.join("inner"), "feat", "main");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).expect("unlock");

        let err = result.expect_err("a path dux cannot read is refused");
        let message = format!("{err:#}");
        assert!(message.contains("could not read"), "{message}");
        assert!(message.contains("mounted"), "{message}");
    }

    /// The hedged sentence belongs to the managed kind too: a standalone folder
    /// dux could not stat keeps git's own wording, which is about consulting
    /// git, while a managed working copy never asked git anything.
    #[test]
    fn an_unreachable_directory_gets_the_hedged_sentence_for_its_kind() {
        use crate::git::FolderRepoStatus;
        let managed = quiet_reason(FolderRepoStatus::Indeterminate, Path::new("/mnt/wt"), true)
            .expect("an unreachable working copy is quiet");
        assert!(managed.contains("/mnt/wt"), "{managed}");
        assert!(managed.contains("mounted"), "{managed}");
        assert!(!managed.contains("no longer exists"), "{managed}");

        let folder = quiet_reason(
            FolderRepoStatus::Indeterminate,
            Path::new("/mnt/mine"),
            false,
        )
        .expect("an unreachable folder is quiet");
        assert_eq!(folder, FolderRepoStatus::Indeterminate.quiet_reason());
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
        let checked_out = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::CheckedOut,
        );
        assert!(checked_out.contains("/tmp/wt"), "{checked_out}");
        assert!(checked_out.contains("checked out again"), "{checked_out}");
        assert!(checked_out.contains("dormant"), "{checked_out}");
        let minted = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::RecreatedFrom("main".to_string()),
        );
        assert!(minted.contains("recreated from \"main\""), "{minted}");
        assert!(minted.contains("none of the"), "{minted}");
        // The remote arm must not read like either of the other two: the pushed
        // commits are back, and only what was never pushed is gone.
        let from_remote = recreate_success_message(
            "v0",
            Path::new("/tmp/wt"),
            "feat",
            &RecreatedBranch::RecreatedFromRemote("origin/feat".to_string()),
        );
        assert!(
            from_remote.contains("recreated from \"origin/feat\""),
            "{from_remote}"
        );
        assert!(
            from_remote.contains("what had been pushed"),
            "{from_remote}"
        );
        assert!(!from_remote.contains("none of the"), "{from_remote}");
    }
}
