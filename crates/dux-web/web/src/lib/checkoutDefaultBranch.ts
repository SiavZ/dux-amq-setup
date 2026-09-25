// Copy for the "Check out default branch" confirmation on an existing project,
// built as prose (`./prose`) so the web draws the project and the base as chips.

import { type Prose, quotedChip } from "./prose"

/**
 * The confirmation body: what the checkout does, and which branch new
 * worktrees start from before and after it. The server names the default
 * branch only once it looks, so the sentence refers to it generically;
 * `leadingBranch` is the project's recorded base (`ProjectView.leading_branch`),
 * which a successful checkout replaces.
 *
 * dux-core's `checkout_default_branch_confirm_prose` builds the same segments
 * for the terminal UI, and both are pinned by
 * `crates/dux-core/tests/fixtures/prose_cross_language.json`.
 */
export function checkoutDefaultBranchProse(
  projectName: string,
  leadingBranch: string | null,
): Prose {
  const lead: Prose = [
    "This switches the source checkout for ",
    quotedChip(projectName),
    " back to its default branch, moving HEAD in the shared repository.",
  ]
  if (leadingBranch) {
    return [
      ...lead,
      " New worktrees branch from ",
      quotedChip(leadingBranch),
      " now. After the checkout, they branch from the default branch.",
    ]
  }
  return [
    ...lead,
    " After the checkout, new worktrees branch from the default branch.",
  ]
}
