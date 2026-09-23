import { describe, expect, it } from "vitest"

import { checkoutDefaultBranchBody } from "./checkoutDefaultBranch"

// Pinned to the exact text of dux-core's `checkout_default_branch_confirm_body`,
// which the terminal UI prints: both surfaces say the same words.
describe("checkoutDefaultBranchBody", () => {
  it("names the project and its current base, and says the checkout moves it to the default", () => {
    expect(checkoutDefaultBranchBody("dux", "develop")).toBe(
      'This switches the source checkout for "dux" back to its default branch, moving HEAD in the shared repository. New worktrees branch from "develop" now. After the checkout, they branch from the default branch.',
    )
  })

  it("still says where worktrees will branch from when no base is recorded", () => {
    expect(checkoutDefaultBranchBody("dux", null)).toBe(
      'This switches the source checkout for "dux" back to its default branch, moving HEAD in the shared repository. After the checkout, new worktrees branch from the default branch.',
    )
  })
})
