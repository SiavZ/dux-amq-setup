import { describe, expect, it } from "vitest"

import {
  checkoutDefaultBranchBody,
  checkoutDefaultBranchProse,
} from "./checkoutDefaultBranch"
import { proseText, quotedChip } from "./prose"

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

// The web draws the project and its base as chips; the plain-text spelling of
// the same structure is the terminal UI's string pinned above.
describe("checkoutDefaultBranchProse", () => {
  it("marks the project and the base, and spells the terminal UI's string", () => {
    const prose = checkoutDefaultBranchProse("dux", "develop")
    expect(prose).toContainEqual(quotedChip("dux"))
    expect(prose).toContainEqual(quotedChip("develop"))
    expect(proseText(prose)).toBe(checkoutDefaultBranchBody("dux", "develop"))
  })

  it("marks only the project when no base is recorded", () => {
    const prose = checkoutDefaultBranchProse("dux", null)
    expect(prose.filter((s) => typeof s !== "string")).toEqual([
      quotedChip("dux"),
    ])
    expect(proseText(prose)).toBe(checkoutDefaultBranchBody("dux", null))
  })
})
