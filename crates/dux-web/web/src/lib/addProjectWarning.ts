// Branch-warning copy and decision helpers for the add-project pre-flight.
//
// The branch warning's three sentences are the terminal UI's too: dux-core builds
// them in `add_project_prose.rs`, and the segments here are pinned against those
// word for word by the shared fixture both suites read
// (crates/dux-core/tests/fixtures/prose_cross_language.json). A line break inside
// one of them is where the terminal UI breaks its row; HTML renders it as a space.
//
// The no-commits, init-repository and inside-a-repository sentences are this
// dialog's own wording: the terminal UI words those dialogs differently, so no
// parity is claimed for them.

import { chip, type Prose, proseText, quotedChip } from "./prose"
import type { BranchWarningView, InspectKind } from "./types"

export type WorktreeNoteTone = "neutral" | "warning"

export interface BranchWarningCopy {
  // The headline sentence describing the situation.
  message: string
  // The same sentence with its branch names marked, for the web to chip.
  messageProse: Prose
  // The always-present note naming the branch new worktrees will fork from.
  worktreeNote: string
  worktreeNoteProse: Prose
  // "neutral" when that branch is the remote default, "warning" otherwise.
  worktreeTone: WorktreeNoteTone
  // The dim explanatory note shown only on the heuristic path; null otherwise.
  heuristicNote: string | null
  // True when the warning offers a "check out the default branch first" action
  // (only the Known variant, matching the TUI's checkbox availability).
  canCheckoutDefault: boolean
  // The default branch name, present only on the Known variant.
  defaultBranch: string | null
}

/** The warning's headline: `defaultBranch` null means dux could not identify one. */
export function branchWarningProse(
  currentBranch: string,
  defaultBranch: string | null,
): Prose {
  return defaultBranch !== null
    ? [
        "This repository is on branch ",
        chip(currentBranch),
        ", but the\nremote default branch is ",
        chip(defaultBranch),
        ".",
      ]
    : [
        "This repository is on branch ",
        chip(currentBranch),
        ",\nwhich doesn't appear to be the main branch.",
      ]
}

/** The note naming the branch new worktrees will start from. */
export function worktreeBaseNoteProse(branch: string): Prose {
  return ["New worktrees will branch from ", quotedChip(branch), "."]
}

/** The dim note under a warning dux could not be sure of. */
export const HEURISTIC_BRANCH_NOTE_PROSE: Prose = [
  "Dux can't confidently identify this repo's default\nbranch, so it won't change branches for you.",
]

/**
 * Map a branch warning + current branch to the exact user-facing copy and the
 * available choices, mirroring the TUI's `ConfirmNonDefaultBranch` rendering.
 * `checkoutSelected` is the "Check out … before adding" box: it decides which
 * branch new worktrees start from, the same rule the server records
 * (`project_base_at_add` in dux-core), so the sentence follows the box live.
 */
export function branchWarningCopy(
  warning: BranchWarningView,
  currentBranch: string,
  checkoutSelected: boolean,
): BranchWarningCopy {
  if (warning.kind === "known") {
    const fromDefault = checkoutSelected
    const messageProse = branchWarningProse(currentBranch, warning.default_branch)
    const worktreeNoteProse = worktreeBaseNoteProse(
      fromDefault ? warning.default_branch : currentBranch,
    )
    return {
      message: proseText(messageProse),
      messageProse,
      worktreeNote: proseText(worktreeNoteProse),
      worktreeNoteProse,
      worktreeTone: fromDefault ? "neutral" : "warning",
      heuristicNote: null,
      canCheckoutDefault: true,
      defaultBranch: warning.default_branch,
    }
  }
  const messageProse = branchWarningProse(currentBranch, null)
  const worktreeNoteProse = worktreeBaseNoteProse(currentBranch)
  return {
    message: proseText(messageProse),
    messageProse,
    worktreeNote: proseText(worktreeNoteProse),
    worktreeNoteProse,
    worktreeTone: "warning",
    heuristicNote: proseText(HEURISTIC_BRANCH_NOTE_PROSE),
    canCheckoutDefault: false,
    defaultBranch: null,
  }
}

export interface NoCommitsCopy {
  // Headline: the repo has no commits yet.
  message: string
  // Reassurance: the commit dux makes is empty and won't touch existing files.
  note: string
}

/**
 * Copy for the unborn-HEAD case: a repo with no commits cannot back a worktree
 * until it has a root commit, so dux offers an empty initial commit.
 */
export function noCommitsCopy(): NoCommitsCopy {
  return {
    message:
      "This repository has no commits yet, so agents can't branch worktrees from it.",
    note: "Dux will make an empty initial commit; your existing files are left untouched (untracked).",
  }
}

export interface InitRepoCopy {
  // Headline: the folder is not a git repository.
  message: string
  // What dux will do: init, seed (when candidates exist), empty commit.
  note: string
  // The same note with each seeded candidate marked, for the web to chip.
  noteProse: Prose
}

/**
 * Copy for the plain-folder case: dux offers to initialize a repository. The seed
 * clause is omitted when there are no candidates, never promising a seed that will not happen.
 */
export function initRepoCopy(candidates: string[]): InitRepoCopy {
  const seedClause: Prose =
    candidates.length > 0
      ? [
          ", seed a starter .gitignore covering ",
          ...candidates.flatMap((candidate, index): Prose =>
            index === 0 ? [chip(candidate)] : [", ", chip(candidate)],
          ),
          ",",
        ]
      : []
  const noteProse: Prose = [
    "Dux will run git init",
    ...seedClause,
    " and make an empty initial commit; your existing files are left untouched (untracked).",
  ]
  return {
    message: "This folder is not a git repository.",
    note: proseText(noteProse),
    noteProse,
  }
}

export interface InsideRepoCopy {
  message: string
  // The same sentence with the enclosing root marked, for the web to chip.
  messageProse: Prose
}

/**
 * Copy for the blocked case: the folder sits inside an existing repository. A null
 * `root` means git's internal directory, and the copy degrades to not naming a root.
 */
export function insideRepoCopy(root: string | null): InsideRepoCopy {
  const messageProse: Prose = root
    ? [
        "This folder is inside the git repository at ",
        chip(root),
        ". Add that repository instead.",
      ]
    : [
        "This folder is inside a git repository's internal directory. Add the repository itself instead.",
      ]
  return { message: proseText(messageProse), messageProse }
}

export type AddProjectAction =
  | "plain"
  | "checkout-default"
  | "initial-commit"
  | "init-repo"
  | "blocked"

export interface AddProjectPrimaryAction {
  action: AddProjectAction
  label: string
}

/**
 * The add dialog's primary action and button label, in precedence order:
 * `blocked` (a folder inside a repository) outranks everything; a `plain` kind
 * outranks `hasCommits`, because init subsumes the commit; an unborn repo
 * outranks a branch warning, because there is no default branch to check out.
 */
export function addProjectPrimaryAction(opts: {
  kind: InspectKind
  hasCommits: boolean
  willCheckout: boolean
  hasBranchWarning: boolean
}): AddProjectPrimaryAction {
  if (opts.kind === "repo_subdir") {
    return { action: "blocked", label: "Add project" }
  }
  if (opts.kind === "plain") {
    return { action: "init-repo", label: "Initialize Repository & Add" }
  }
  if (!opts.hasCommits) {
    return { action: "initial-commit", label: "Create Initial Commit & Add" }
  }
  if (opts.willCheckout) {
    return { action: "checkout-default", label: "Check Out & Add" }
  }
  if (opts.hasBranchWarning) {
    return { action: "plain", label: "Add Anyway" }
  }
  return { action: "plain", label: "Add project" }
}
