// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"
import type { ReactNode } from "react"

import { PrBanner } from "@/components/PrBanner"
import type { PrView } from "@/lib/types"
import { type RawWorkspace, normalizeWorkspace } from "@/lib/workspaceApi"

// The real tooltip only mounts its popup into a portal on hover and needs a
// ResizeObserver, which jsdom lacks. Render its `content` inline instead so a
// test can assert what the banner's tooltip is wired to reveal, mirroring the
// pattern used in Sidebar.test.tsx.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: ReactNode
    content: ReactNode
  }) => (
    <>
      {children}
      <div data-testid="tooltip-content">{content}</div>
    </>
  ),
}))

function pr(overrides: Partial<PrView> = {}): PrView {
  return {
    number: 42,
    state: "open",
    title: "A very long pull request title that would truncate in the banner",
    url: "https://example.com/pr/42",
    ...overrides,
  }
}

describe("PrBanner", () => {
  afterEach(() => {
    cleanup()
  })

  it("renders the visible #number once, not repeated in the tooltip", () => {
    render(<PrBanner pr={pr()} />)

    // The number is visible in the banner itself.
    expect(screen.getByText("#42")).toBeTruthy()
    // The tooltip content carries only the title, not another "#42".
    const tooltip = screen.getByTestId("tooltip-content")
    expect(tooltip.textContent).not.toContain("#42")
  })

  it("carries the full PR title in the tooltip so a truncated title stays readable", () => {
    const longTitle =
      "A very long pull request title that would truncate in the banner"
    render(<PrBanner pr={pr({ title: longTitle })} />)

    const tooltip = screen.getByTestId("tooltip-content")
    expect(tooltip.textContent).toBe(longTitle)
  })
})

describe("PrBanner with a title that arrived over the wire", () => {
  afterEach(() => {
    cleanup()
  })

  it("draws the title in its stored order, with no bidi control left in it", () => {
    const spine = normalizeWorkspace({
      projects: [],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          pr: pr({ title: "Fix login \u202Eexe.txt\u202C" }),
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    } as unknown as RawWorkspace)
    const { container } = render(<PrBanner pr={spine.sessions[0].pr as PrView} />)
    expect(container.textContent).toContain("Fix login exe.txt")
    expect(container.textContent).not.toMatch(/[\u202A-\u202E\u2066-\u2069\u200E\u200F\u061C]/)
  })
})
