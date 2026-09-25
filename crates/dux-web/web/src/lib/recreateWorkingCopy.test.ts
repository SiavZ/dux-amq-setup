import { describe, expect, it } from "vitest"

import type { AgentWorkspaceWire } from "./agentWorkspace"
import type { SessionView } from "./types"
import {
  canRecreateWorkingCopy,
  recreateConfirmBody,
  recreateRunningProviders,
  recreateRunningTabClause,
} from "./recreateWorkingCopy"

const managed: AgentWorkspaceWire = {
  kind: "managed",
  project_id: "p1",
  branch_name: "feat",
  initial_branch: "feat",
  branch_provenance: "created",
  source_branch: "main",
  worktree_path: "/wt",
}

describe("canRecreateWorkingCopy", () => {
  it("is true only for a managed working copy that is gone", () => {
    expect(canRecreateWorkingCopy(managed)).toBe(false)
    expect(
      canRecreateWorkingCopy({ ...managed, worktree_missing: true }),
    ).toBe(true)
  })

  // dux never creates, moves or removes a standalone agent's folder.
  it("is false for a standalone agent whatever its folder looks like", () => {
    const folder: AgentWorkspaceWire = {
      kind: "folder",
      folder_path: "/home/someone/notes",
      folder_label: "~/notes",
      repo_status: "missing",
      quiet_reason: "gone",
    }
    expect(canRecreateWorkingCopy(folder)).toBe(false)
  })
})

describe("recreateConfirmBody", () => {
  // Mirrors `dux_core::working_copy::recreate_confirm_body`. The Rust side has
  // the twin assertions, so a wording change fails on whichever side changed.
  it("reads the same as the terminal UI's", () => {
    expect(
      recreateConfirmBody("~/worktrees/repo/feat", "feat", "main", true, [
        "claude",
      ]),
    ).toBe(
      "Recreate the working copy for this agent at ~/worktrees/repo/feat?\n\n" +
        'If branch "feat" still exists locally, dux checks it out there again. ' +
        'If it is gone locally but still on the remote, dux creates it again ' +
        'from "origin/feat", holding everything that had been pushed. If it is ' +
        'gone everywhere, dux creates it again from "main", and the commits ' +
        "that branch held are not coming back.\n\n" +
        "Any code changes that were in the old directory are gone either way: " +
        "this puts the directory back, not its contents. The conversation may " +
        "resume, because the agent's CLI keys its history by directory path and " +
        "dux recreates the working copy at the same path.\n\n" +
        "A running Claude tab keeps working in the recreated copy by itself. " +
        "A terminal still open in the old directory keeps " +
        "working in a directory that is gone; close it and open one in the " +
        "recreated copy.",
    )
  })

  // A provider with no directory-scoped resume (copilot ships with none) is
  // told so rather than promised a resume the same path cannot buy it.
  it("says the conversation will not resume when the provider cannot", () => {
    const body = recreateConfirmBody("~/wt", "feat", "main", false, ["copilot"])
    expect(body).toContain("will not resume")
    expect(body).not.toContain("may resume")
    expect(body).toContain("are gone either way")
  })

  // Measured per CLI: Claude follows the folder on its own, Codex is stuck
  // until it is quit and resumed, and an unmeasured provider gets the cautious
  // answer.
  it("says what a running tab does, per provider", () => {
    expect(
      recreateConfirmBody("~/wt", "feat", "main", true, ["claude"]),
    ).toContain("A running Claude tab keeps working in the recreated copy by itself.")
    for (const provider of ["codex", "opencode", "copilot"]) {
      const body = recreateConfirmBody("~/wt", "feat", "main", true, [provider])
      expect(body).toContain(
        "cannot follow the folder: stop it and start the agent again",
      )
      expect(body).not.toContain("keeps working in the recreated copy")
    }
    expect(recreateRunningTabClause(["codex"])).toBe(
      "A running Codex tab cannot follow the folder: stop it and start the " +
        "agent again to continue in the recreated copy.",
    )
  })

  // One stuck CLI among the running tabs makes the whole sentence the cautious
  // one, and it names every tab the user has to stop.
  it("is cautious when any running tab is not claude", () => {
    expect(recreateRunningTabClause(["claude", "codex"])).toBe(
      "A running Codex tab cannot follow the folder: stop it and start the " +
        "agent again to continue in the recreated copy.",
    )
    expect(recreateRunningTabClause(["codex", "opencode"])).toBe(
      "A running Codex or Opencode tab cannot follow the folder: stop it and " +
        "start the agent again to continue in the recreated copy.",
    )
    expect(recreateRunningTabClause(["codex", "opencode", "copilot"])).toBe(
      "A running Codex, Opencode or Copilot tab cannot follow the folder: " +
        "stop it and start the agent again to continue in the recreated copy.",
    )
    expect(recreateRunningTabClause(["claude"])).toBe(
      "A running Claude tab keeps working in the recreated copy by itself.",
    )
  })
})

describe("recreateRunningProviders", () => {
  const session = (
    tabs: { provider: string; has_live_process: boolean }[],
  ): SessionView =>
    ({ provider: "claude", tabs }) as unknown as SessionView

  // The sentence is about the CLI that is running, and the agent's own provider
  // is only the slot tab's.
  it("names the live tabs, not the agent's provider mirror", () => {
    expect(
      recreateRunningProviders(
        session([
          { provider: "claude", has_live_process: false },
          { provider: "codex", has_live_process: true },
        ]),
      ),
    ).toEqual(["codex"])

    expect(
      recreateRunningProviders(
        session([
          { provider: "claude", has_live_process: true },
          { provider: "codex", has_live_process: true },
          { provider: "codex", has_live_process: true },
        ]),
      ),
    ).toEqual(["claude", "codex"])
  })

  // With nothing running the dialog still says what starting a tab would mean.
  it("falls back to the agent's own provider when nothing runs", () => {
    expect(
      recreateRunningProviders(
        session([{ provider: "codex", has_live_process: false }]),
      ),
    ).toEqual(["claude"])
    expect(recreateRunningProviders(session([]))).toEqual(["claude"])
  })
})
