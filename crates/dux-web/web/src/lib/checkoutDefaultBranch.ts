// Copy for the "Checkout default branch" confirmation on an existing project.
// Byte-for-byte dux-core's `checkout_default_branch_confirm_body`, which the
// terminal UI's confirmation prints: keep the two in step.

/**
 * The confirmation body: what the checkout does, and which branch new
 * worktrees start from before and after it. The server names the default
 * branch only once it looks, so the sentence refers to it generically;
 * `leadingBranch` is the project's recorded base (`ProjectView.leading_branch`),
 * which a successful checkout replaces.
 */
export function checkoutDefaultBranchBody(
  projectName: string,
  leadingBranch: string | null,
): string {
  const lead = `This switches the source checkout for "${projectName}" back to its default branch, moving HEAD in the shared repository.`
  if (leadingBranch) {
    return `${lead} New worktrees branch from "${leadingBranch}" now. After the checkout, they branch from the default branch.`
  }
  return `${lead} After the checkout, new worktrees branch from the default branch.`
}
