import { describe, expect, it } from "vitest"

import { checkoutDefaultBranchProse } from "./checkoutDefaultBranch"
import { proseText, quotedChip } from "./prose"

// The plain-text spelling is pinned to literal strings; the segments are pinned
// against the terminal UI's by the shared fixture in prose.test.tsx.
describe("checkoutDefaultBranchProse", () => {
  it("names the project and its current base, and says the checkout moves it to the default", () => {
    const prose = checkoutDefaultBranchProse("dux", "develop")
    expect(prose).toContainEqual(quotedChip("dux"))
    expect(prose).toContainEqual(quotedChip("develop"))
    expect(proseText(prose)).toBe(
      'This switches the source checkout for "dux" back to its default branch, moving HEAD in the shared repository. New worktrees branch from "develop" now. After the checkout, they branch from the default branch.',
    )
  })

  it("marks only the project, and still says where worktrees will branch from, when no base is recorded", () => {
    const prose = checkoutDefaultBranchProse("dux", null)
    expect(prose.filter((s) => typeof s !== "string")).toEqual([
      quotedChip("dux"),
    ])
    expect(proseText(prose)).toBe(
      'This switches the source checkout for "dux" back to its default branch, moving HEAD in the shared repository. After the checkout, new worktrees branch from the default branch.',
    )
  })
})
