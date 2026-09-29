// Copy for the "Delete project?" and "Remove project?" confirmations, built as
// prose (`./prose`) so the web draws the project name as a chip.
//
// dux-core's `project_prose` builds the same segments for the terminal UI, and
// both are pinned by `crates/dux-core/tests/fixtures/prose_cross_language.json`.

import { formatRegularCount } from "./formatRegularCount"
import { type Prose, quotedChip } from "./prose"

// `name` is undefined only where the dialog has no name to show; the sentence
// then says "this project" rather than inventing one.
function project(name: string | undefined): Prose {
  return name !== undefined ? [quotedChip(name)] : ["this project"]
}

/**
 * The worktree-deleting cascade: the project, every agent in it and their
 * worktrees go, and the source checkout stays. The agents and worktrees are
 * named only when there are any.
 */
export function deleteProjectProse(
  name: string | undefined,
  agentCount: number,
): Prose {
  const cascade =
    agentCount > 0
      ? `, its ${formatRegularCount(agentCount, "agent")}, and ${
          agentCount === 1 ? "its worktree" : "their worktrees"
        } on disk`
      : ""
  return [
    "This deletes ",
    ...project(name),
    `${cascade} from dux. This is irreversible. The source checkout is kept.`,
  ]
}

/**
 * The record-only removal: the project leaves dux, any agents it still holds
 * leave with it, and every worktree stays on disk.
 */
export function removeProjectProse(
  name: string | undefined,
  agentCount: number,
): Prose {
  const agents =
    agentCount > 0 ? ` and deletes its ${formatRegularCount(agentCount, "agent")}` : ""
  return [
    "This removes ",
    ...project(name),
    `${agents} from dux. Worktrees on disk are kept.`,
  ]
}
