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

use crate::home_path::shorten_home;

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
pub fn recreate_confirm_body(worktree: &Path, branch_name: &str, branch_exists: bool) -> String {
    let branch_line = if branch_exists {
        format!("Branch \"{branch_name}\" still exists, so dux checks it out again at that path.")
    } else {
        format!(
            "Branch \"{branch_name}\" is gone too, so dux creates it again from the project's \
             source branch. The commits that branch held are not coming back."
        )
    };
    format!(
        "Recreate the working copy for this agent at {}?\n\n{branch_line}\n\nAny code changes \
         that were in the old directory are gone: this puts the directory back, not its \
         contents. The conversation may resume, because the agent's CLI keys its history by \
         directory path and dux recreates the working copy at the same path.\n\nAnything still \
         running keeps working in the deleted directory; stop it and start the agent again to \
         work in the recreated copy.",
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

    #[test]
    fn the_confirm_says_the_changes_are_gone_on_both_branch_outcomes() {
        let kept = recreate_confirm_body(Path::new("/tmp/wt"), "feat", true);
        assert!(kept.contains("checks it out again"), "{kept}");
        assert!(kept.contains("are gone"), "{kept}");
        assert!(kept.contains("same path"), "{kept}");
        let minted = recreate_confirm_body(Path::new("/tmp/wt"), "feat", false);
        assert!(minted.contains("source branch"), "{minted}");
        assert!(minted.contains("not coming back"), "{minted}");
        assert!(minted.contains("deleted directory"), "{minted}");
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
