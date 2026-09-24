//! The sentences the add-project branch warning prints, built once as prose
//! for both surfaces.
//!
//! [`crate::add_project_plan`] decides WHICH warning fires; these builders say
//! it. The terminal UI renders them in its Non-Default Branch dialog and the
//! browser's `lib/addProjectWarning.ts` builds the same segments, pinned
//! against each other by `tests/fixtures/prose_cross_language.json`.
//!
//! A single line break inside a sentence is where the terminal UI's dialog
//! breaks the row; the web renders it as the space HTML makes of any line
//! break.

use crate::prose::Prose;

/// The headline of the warning: the branch the repository is on, and either
/// the remote's default branch (`Some`) or that dux could not identify one.
pub fn branch_warning_prose(current_branch: &str, default_branch: Option<&str>) -> Prose {
    let lead = Prose::new()
        .text("This repository is on branch ")
        .name(current_branch);
    match default_branch {
        Some(default_branch) => lead
            .text(", but the\nremote default branch is ")
            .name(default_branch)
            .text("."),
        None => lead.text(",\nwhich doesn't appear to be the main branch."),
    }
}

/// The note naming the branch new worktrees will start from.
pub fn worktree_base_note_prose(branch: &str) -> Prose {
    Prose::new()
        .text("New worktrees will branch from ")
        .quoted(branch)
        .text(".")
}

/// The dim note under a warning dux could not be sure of: it names nothing,
/// but both surfaces print it word for word.
pub fn heuristic_branch_note_prose() -> Prose {
    Prose::new().text(
        "Dux can't confidently identify this repo's default\nbranch, so it won't change \
         branches for you.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_known_warning_names_both_branches_and_breaks_where_the_dialog_does() {
        assert_eq!(
            branch_warning_prose("feature/x", Some("main")).plain(),
            "This repository is on branch feature/x, but the\nremote default branch is main."
        );
    }

    #[test]
    fn the_heuristic_warning_names_only_the_current_branch() {
        assert_eq!(
            branch_warning_prose("dev", None).plain(),
            "This repository is on branch dev,\nwhich doesn't appear to be the main branch."
        );
    }

    #[test]
    fn the_worktree_note_quotes_the_branch_in_its_plain_spelling() {
        assert_eq!(
            worktree_base_note_prose("main").plain(),
            "New worktrees will branch from \"main\"."
        );
    }
}
