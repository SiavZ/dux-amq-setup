//! The bodies of the "delete project?" and "remove project?" confirmations,
//! built once as prose for both surfaces.
//!
//! The terminal UI renders them in its Confirm Delete Project and Confirm
//! Remove Project dialogs, and the browser's `lib/projectConfirm.ts` builds the
//! same segments for `DeleteProjectDialog` and `RemoveProjectDialog`. The two
//! are pinned against each other by `tests/fixtures/prose_cross_language.json`.
//!
//! `project_name` is `None` only where a surface has no name to show; the
//! sentence then says "this project" rather than inventing one.

use crate::prose::Prose;
use crate::text::count_of;

fn project(prose: Prose, project_name: Option<&str>) -> Prose {
    match project_name {
        Some(name) => prose.quoted(name),
        None => prose.text("this project"),
    }
}

/// The worktree-deleting cascade: the project, every agent in it and their
/// worktrees go, and the source checkout stays. The agents and worktrees are
/// named only when there are any, because a project with no agents has no
/// worktrees to mention.
pub fn delete_project_confirm_prose(project_name: Option<&str>, agent_count: usize) -> Prose {
    let lead = project(Prose::new().text("This deletes "), project_name);
    let lead = if agent_count > 0 {
        let worktrees = if agent_count == 1 {
            "its worktree"
        } else {
            "their worktrees"
        };
        lead.text(format!(
            ", its {}, and {worktrees} on disk",
            count_of(agent_count, "agent")
        ))
    } else {
        lead
    };
    lead.text(" from dux. This is irreversible. The source checkout is kept.")
}

/// The record-only removal: the project leaves dux, any agents it still holds
/// leave with it, and every worktree stays on disk.
pub fn remove_project_confirm_prose(project_name: Option<&str>, agent_count: usize) -> Prose {
    let lead = project(Prose::new().text("This removes "), project_name);
    let lead = if agent_count > 0 {
        lead.text(format!(
            " and deletes its {}",
            count_of(agent_count, "agent")
        ))
    } else {
        lead
    };
    lead.text(" from dux. Worktrees on disk are kept.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delete_body_names_the_cascade_only_when_there_is_one() {
        assert_eq!(
            delete_project_confirm_prose(Some("dux"), 0).plain(),
            "This deletes \"dux\" from dux. This is irreversible. The source checkout is kept."
        );
        assert_eq!(
            delete_project_confirm_prose(Some("dux"), 2).plain(),
            "This deletes \"dux\", its 2 agents, and their worktrees on disk from dux. This is \
             irreversible. The source checkout is kept."
        );
    }

    #[test]
    fn the_remove_body_keeps_the_worktrees() {
        assert_eq!(
            remove_project_confirm_prose(Some("dux"), 1).plain(),
            "This removes \"dux\" and deletes its 1 agent from dux. Worktrees on disk are kept."
        );
        assert_eq!(
            remove_project_confirm_prose(None, 0).plain(),
            "This removes this project from dux. Worktrees on disk are kept."
        );
    }
}
