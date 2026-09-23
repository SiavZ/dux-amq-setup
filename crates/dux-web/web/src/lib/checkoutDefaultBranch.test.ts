import { describe, expect, it } from "vitest"

import { checkoutDefaultBaseNote } from "./checkoutDefaultBranch"

describe("checkoutDefaultBaseNote", () => {
  it("names the project's current base and says the checkout moves it to the default", () => {
    expect(checkoutDefaultBaseNote("feature/x")).toBe(
      'New worktrees branch from "feature/x" now. After the checkout, they branch from the default branch.',
    )
  })

  it("still says where worktrees will branch from when no base is recorded", () => {
    expect(checkoutDefaultBaseNote(null)).toBe(
      "After the checkout, new worktrees branch from the default branch.",
    )
  })
})
