// Copy for the "Checkout default branch" confirmation on an existing project.
// Web-only: the terminal UI runs this command without a dialog, so there is no
// TUI string to stay byte-for-byte with.

/**
 * The line saying which branch new worktrees start from, before and after the
 * checkout. The server names the default branch only once it looks, so the
 * sentence refers to it generically; `leadingBranch` is the project's recorded
 * base (`ProjectView.leading_branch`), which a successful checkout replaces.
 */
export function checkoutDefaultBaseNote(leadingBranch: string | null): string {
  if (leadingBranch) {
    return `New worktrees branch from "${leadingBranch}" now. After the checkout, they branch from the default branch.`
  }
  return "After the checkout, new worktrees branch from the default branch."
}
