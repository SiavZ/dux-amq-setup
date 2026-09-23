// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"

let mockState: DuxState
vi.mock("@/lib/store", () => ({
  useDux: () => mockState,
  checkoutDefaultBranch: vi.fn(),
  closeCheckoutDefaultBranch: vi.fn(),
}))

import { CheckoutDefaultBranchDialog } from "@/components/CheckoutDefaultBranchDialog"

afterEach(() => cleanup())

describe("CheckoutDefaultBranchDialog", () => {
  it("asks with the terminal UI's words and its confirm label", () => {
    mockState = {
      checkoutDefaultBranchTarget: "p1",
      spine: { projects: [{ id: "p1", name: "dux", leading_branch: "develop" }] },
    } as unknown as DuxState
    render(<CheckoutDefaultBranchDialog />)

    expect(
      screen.getByText(
        'This switches the source checkout for "dux" back to its default branch, moving HEAD in the shared repository. New worktrees branch from "develop" now. After the checkout, they branch from the default branch.',
      ),
    ).toBeTruthy()
    // The terminal UI's button reads the same, in the verb form.
    expect(
      screen.getByRole("button", { name: "Check out default branch" }),
    ).toBeTruthy()
  })
})
