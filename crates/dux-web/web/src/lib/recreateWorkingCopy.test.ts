import { describe, expect, it } from "vitest"

import type { AgentWorkspaceWire } from "./agentWorkspace"
import {
  canRecreateWorkingCopy,
  recreateConfirmBody,
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
    expect(recreateConfirmBody("~/worktrees/repo/feat", "feat", "main")).toBe(
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
        "Its tabs are dormant and stay that way, because dux refuses this while " +
        "the agent is running. A terminal still open in the old directory keeps " +
        "working in a directory that is gone; close it and open one in the " +
        "recreated copy.",
    )
  })
})
