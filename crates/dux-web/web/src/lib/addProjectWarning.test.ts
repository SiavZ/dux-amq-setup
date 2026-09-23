import { describe, expect, it } from "vitest"

import {
  addProjectPrimaryAction,
  branchWarningCopy,
  initRepoCopy,
  insideRepoCopy,
  noCommitsCopy,
} from "./addProjectWarning"
import { chip, proseText, quotedChip } from "./prose"

describe("branchWarningCopy", () => {
  it("names the default branch and offers checkout for a known warning", () => {
    const copy = branchWarningCopy(
      { kind: "known", default_branch: "main" },
      "feature/x",
      true,
    )
    expect(copy.message).toBe(
      "This repository is on branch feature/x, but the remote default branch is main.",
    )
    expect(copy.heuristicNote).toBeNull()
    expect(copy.canCheckoutDefault).toBe(true)
    expect(copy.defaultBranch).toBe("main")
  })

  it("says worktrees branch from the default, calmly, while the checkout is ticked", () => {
    const copy = branchWarningCopy(
      { kind: "known", default_branch: "main" },
      "feature/x",
      true,
    )
    expect(copy.worktreeNote).toBe('New worktrees will branch from "main".')
    expect(copy.worktreeTone).toBe("neutral")
  })

  it("warns that worktrees branch from the current branch once the checkout is unticked", () => {
    const copy = branchWarningCopy(
      { kind: "known", default_branch: "main" },
      "feature/x",
      false,
    )
    expect(copy.worktreeNote).toBe('New worktrees will branch from "feature/x".')
    expect(copy.worktreeTone).toBe("warning")
  })

  it("warns without offering checkout for a heuristic warning", () => {
    // The box does not exist on this path, so a stale "ticked" changes nothing.
    const copy = branchWarningCopy({ kind: "heuristic" }, "dev", true)
    expect(copy.message).toBe(
      "This repository is on branch dev, which doesn't appear to be the main branch.",
    )
    expect(copy.worktreeNote).toBe('New worktrees will branch from "dev".')
    expect(copy.worktreeTone).toBe("warning")
    expect(copy.heuristicNote).toBe(
      "Dux can't confidently identify this repo's default branch, so it won't change branches for you.",
    )
    expect(copy.canCheckoutDefault).toBe(false)
    expect(copy.defaultBranch).toBeNull()
  })
})

// The web draws every name as a chip, so each sentence also comes as structure.
// Its plain-text spelling is the very string pinned above, which is what keeps
// the terminal UI's words intact while the web drops the quotes.
describe("the add-project copy as names in prose", () => {
  it("marks both branches in the known warning, and spells the TUI's string", () => {
    const copy = branchWarningCopy(
      { kind: "known", default_branch: "main" },
      "feature/x",
      false,
    )
    expect(copy.messageProse).toEqual([
      "This repository is on branch ",
      chip("feature/x"),
      ", but the remote default branch is ",
      chip("main"),
      ".",
    ])
    expect(proseText(copy.messageProse)).toBe(copy.message)
    expect(copy.worktreeNoteProse).toEqual([
      "New worktrees will branch from ",
      quotedChip("feature/x"),
      ".",
    ])
    expect(proseText(copy.worktreeNoteProse)).toBe(copy.worktreeNote)
  })

  it("marks the branch in the heuristic warning", () => {
    const copy = branchWarningCopy({ kind: "heuristic" }, "dev", true)
    expect(copy.messageProse).toContainEqual(chip("dev"))
    expect(proseText(copy.messageProse)).toBe(copy.message)
  })

  it("marks each seeded candidate and the enclosing root", () => {
    const init = initRepoCopy(["node_modules", ".venv"])
    expect(init.noteProse).toContainEqual(chip("node_modules"))
    expect(init.noteProse).toContainEqual(chip(".venv"))
    expect(proseText(init.noteProse)).toBe(init.note)
    expect(proseText(initRepoCopy([]).noteProse)).toBe(initRepoCopy([]).note)

    const inside = insideRepoCopy("/home/u/repo")
    expect(inside.messageProse).toContainEqual(chip("/home/u/repo"))
    expect(proseText(inside.messageProse)).toBe(inside.message)
    expect(proseText(insideRepoCopy(null).messageProse)).toBe(
      insideRepoCopy(null).message,
    )
  })
})

describe("noCommitsCopy", () => {
  it("explains the repo has no commits and that an empty commit will be made", () => {
    const copy = noCommitsCopy()
    expect(copy.message).toContain("no commits")
    // The commit is empty and leaves existing files untouched/untracked.
    expect(copy.note).toContain("empty")
  })
})

describe("initRepoCopy", () => {
  it("names the candidates dux will seed", () => {
    const copy = initRepoCopy(["node_modules", ".venv"])
    expect(copy.message).toBe("This folder is not a git repository.")
    expect(copy.note).toContain("git init")
    expect(copy.note).toContain("node_modules, .venv")
    expect(copy.note).toContain("empty initial commit")
  })

  it("omits the seed clause entirely when there are no candidates", () => {
    // Never promise a seed that will not happen.
    const copy = initRepoCopy([])
    expect(copy.note).not.toContain(".gitignore")
    expect(copy.note).toContain("git init")
  })
})

describe("insideRepoCopy", () => {
  it("names the enclosing repository root", () => {
    const copy = insideRepoCopy("/home/u/repo")
    expect(copy.message).toBe(
      "This folder is inside the git repository at /home/u/repo. Add that repository instead.",
    )
  })

  it("degrades gracefully when no root can be named (git-internal dir)", () => {
    const copy = insideRepoCopy(null)
    expect(copy.message).toContain("internal directory")
    expect(copy.message).not.toContain("null")
  })
})

describe("addProjectPrimaryAction", () => {
  it("blocks a repo subdirectory, outranking everything (even hasCommits: false)", () => {
    // A wrong rung here means the wrong wire flag, i.e. the wrong server
    // mutation (an initial commit inside someone's repo subfolder).
    const action = addProjectPrimaryAction({
      kind: "repo_subdir",
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("blocked")
  })

  it("offers to initialize a repository for a plain folder", () => {
    const action = addProjectPrimaryAction({
      kind: "plain",
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("init-repo")
    expect(action.label).toBe("Initialize Repository & Add")
  })

  it("offers to create the initial commit when the repo has none, taking precedence over branch warnings", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: false,
      willCheckout: false,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("initial-commit")
    expect(action.label).toBe("Create Initial Commit & Add")
  })

  it("checks out the default first when the user opted in", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: true,
      willCheckout: true,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("checkout-default")
    expect(action.label).toBe("Check Out & Add")
  })

  it("reads 'Add Anyway' for a branch warning without checkout", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: true,
      willCheckout: false,
      hasBranchWarning: true,
    })
    expect(action.action).toBe("plain")
    expect(action.label).toBe("Add Anyway")
  })

  it("reads 'Add project' for a clean repo on its default branch", () => {
    const action = addProjectPrimaryAction({
      kind: "repo",
      hasCommits: true,
      willCheckout: false,
      hasBranchWarning: false,
    })
    expect(action.action).toBe("plain")
    expect(action.label).toBe("Add project")
  })
})
