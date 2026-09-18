// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { DuxState } from "@/lib/store"
import type { SessionView } from "@/lib/types"

const closeRecreateWorkingCopy = vi.fn()
const recreateWorkingCopy = vi.fn()

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeRecreateWorkingCopy: () => closeRecreateWorkingCopy(),
    recreateWorkingCopy: (s: string) => recreateWorkingCopy(s),
  }
})

function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
}
installBootStubs()
const { ConfirmRecreateWorkingCopyDialog } = await import(
  "./ConfirmRecreateWorkingCopyDialog"
)

function session(missing: boolean): SessionView {
  return {
    id: "s1",
    workspace: {
      kind: "managed",
      project_id: "p1",
      branch_name: "feat",
      initial_branch: "feat",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/home/someone/.config/dux/worktrees/repo/feat",
      worktree_label: "~/.config/dux/worktrees/repo/feat",
      worktree_missing: missing,
      quiet_reason: missing ? "The working copy no longer exists on disk." : "",
      // The fixture agent runs claude, which resumes per directory.
      conversation_resumes: true,
    },
    title: null,
    provider: "claude",
    status: "active",
    auto_reopen_enabled: false,
    tabs: [],
    has_output: false,
    working: false,
    needs_attention: false,
    slot_tab_id: "s1",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
  } as unknown as SessionView
}

function seed(over: Partial<DuxState>) {
  mockState = {
    recreateWorkingCopyTarget: null,
    spine: null,
    ...over,
  } as DuxState
}

beforeEach(() => {
  closeRecreateWorkingCopy.mockClear()
  recreateWorkingCopy.mockClear()
})

afterEach(cleanup)

describe("ConfirmRecreateWorkingCopyDialog", () => {
  it("stays shut with no target", () => {
    seed({ recreateWorkingCopyTarget: null })
    render(<ConfirmRecreateWorkingCopyDialog />)
    expect(screen.queryByText("Recreate working copy?")).toBeNull()
  })

  // The three things a user must know before agreeing: where it lands, that the
  // code changes are gone, and that the process still running is left alone.
  it("says what is lost, what may survive, and what keeps running", () => {
    seed({
      recreateWorkingCopyTarget: "s1",
      spine: { sessions: [session(true)], projects: [] },
    } as unknown as Partial<DuxState>)
    render(<ConfirmRecreateWorkingCopyDialog />)

    const body = screen.getByText(/Recreate the working copy/).textContent ?? ""
    expect(body).toContain("~/.config/dux/worktrees/repo/feat")
    expect(body).toContain('branch "feat" still exists')
    expect(body).toContain('from "main"')
    expect(body).toContain("are gone either way")
    expect(body).toContain("same path")
    expect(body).toContain("refuses this while the agent is running")
  })

  it("recreates on confirm and closes itself", () => {
    seed({
      recreateWorkingCopyTarget: "s1",
      spine: { sessions: [session(true)], projects: [] },
    } as unknown as Partial<DuxState>)
    render(<ConfirmRecreateWorkingCopyDialog />)

    fireEvent.click(screen.getByText("Recreate"))
    expect(recreateWorkingCopy).toHaveBeenCalledWith("s1")
    expect(closeRecreateWorkingCopy).toHaveBeenCalled()
  })

  it("cancels without acting, and Cancel is the default focus", () => {
    seed({
      recreateWorkingCopyTarget: "s1",
      spine: { sessions: [session(true)], projects: [] },
    } as unknown as Partial<DuxState>)
    render(<ConfirmRecreateWorkingCopyDialog />)

    expect(document.activeElement?.textContent).toBe("Cancel")
    fireEvent.click(screen.getByText("Cancel"))
    expect(recreateWorkingCopy).not.toHaveBeenCalled()
    expect(closeRecreateWorkingCopy).toHaveBeenCalled()
  })

  // A working copy that came back on its own counts as the target vanishing:
  // the menu entry is gone by then and this dialog has nothing left to do.
  it("closes itself when the working copy comes back", () => {
    seed({
      recreateWorkingCopyTarget: "s1",
      spine: { sessions: [session(false)], projects: [] },
    } as unknown as Partial<DuxState>)
    render(<ConfirmRecreateWorkingCopyDialog />)

    expect(screen.queryByText("Recreate working copy?")).toBeNull()
    expect(closeRecreateWorkingCopy).toHaveBeenCalled()
  })
})
