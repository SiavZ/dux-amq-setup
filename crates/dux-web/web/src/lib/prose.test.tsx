// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"
import { render } from "@testing-library/react"

import {
  branchWarningProse,
  HEURISTIC_BRANCH_NOTE_PROSE,
  worktreeBaseNoteProse,
} from "./addProjectWarning"
import { checkoutDefaultBranchProse } from "./checkoutDefaultBranch"
import { detachConfirmProse } from "./detachAgent"
import { deleteProjectProse, removeProjectProse } from "./projectConfirm"
import {
  chip,
  type Prose,
  type ProseSegment,
  proseText,
  quotedChip,
  renderProse,
  wireProse,
} from "./prose"
import { recreateConfirmProse } from "./recreateWorkingCopy"

describe("a sentence built from prose and names", () => {
  it("renders every name as the shared chip and the rest as text", () => {
    const { container } = render(
      <p>{renderProse(["Delete ", chip("feat/x"), " from ", quotedChip("~/repo"), "?"])}</p>,
    )
    const codes = [...container.querySelectorAll("code")].map((c) => c.textContent)
    expect(codes).toEqual(["feat/x", "~/repo"])
    expect(container.querySelector("code")?.dataset.slot).toBe("inline-code")
    // No quotes on the web: the chip is the delimiter.
    expect(container.textContent).toBe("Delete feat/x from ~/repo?")
  })

  it("spells the plain-text form with quotes only where the name was quoted", () => {
    expect(
      proseText(["on branch ", chip("dev"), ", from ", quotedChip("main"), "."]),
    ).toBe('on branch dev, from "main".')
  })

  it("renders an empty name as an empty chip rather than dropping it", () => {
    const { container } = render(<p>{renderProse(["a ", chip(""), " b"])}</p>)
    expect(container.querySelectorAll("code")).toHaveLength(1)
  })
})

describe("the sentences both surfaces print", () => {
  // The other half of the pin in dux-core's `prose.rs`: both read this same
  // file, so a sentence passes only when the terminal UI and the web say the
  // same words and mark the same names. Adjacent strings are merged first, so
  // either side may split its constant words however it likes.
  const fixture = JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "../../../../dux-core/tests/fixtures/prose_cross_language.json",
      ),
      "utf8",
    ),
  ) as {
    cases: {
      what: string
      sentence: string
      args: Record<string, unknown>
      segments: Prose
    }[]
  }

  function merged(prose: Prose): Prose {
    const out: ProseSegment[] = []
    for (const segment of prose) {
      const last = out[out.length - 1]
      if (typeof segment === "string" && typeof last === "string") {
        out[out.length - 1] = last + segment
      } else {
        out.push(segment)
      }
    }
    return out
  }

  function build(sentence: string, args: Record<string, unknown>): Prose {
    switch (sentence) {
      case "detach_confirm":
        return detachConfirmProse(
          args.label as string,
          args.grace_seconds as number,
          args.live_tabs as number,
        )
      case "recreate_confirm":
        return recreateConfirmProse(
          args.worktree_label as string,
          args.branch_name as string,
          args.source_branch as string,
          args.conversation_resumes as boolean,
          args.providers as string[],
        )
      case "checkout_default_branch_confirm":
        return checkoutDefaultBranchProse(
          args.project_name as string,
          args.stored_base as string | null,
        )
      case "add_project_branch_warning":
        return branchWarningProse(
          args.current_branch as string,
          args.default_branch as string | null,
        )
      case "add_project_worktree_base":
        return worktreeBaseNoteProse(args.branch as string)
      case "add_project_heuristic_note":
        return HEURISTIC_BRANCH_NOTE_PROSE
      case "delete_project_confirm":
        return deleteProjectProse(
          (args.project_name as string | null) ?? undefined,
          args.agent_count as number,
        )
      case "remove_project_confirm":
        return removeProjectProse(
          (args.project_name as string | null) ?? undefined,
          args.agent_count as number,
        )
      default:
        throw new Error(`the fixture names a sentence this test cannot build: ${sentence}`)
    }
  }

  it("has not lost its cases", () => {
    expect(fixture.cases.length).toBeGreaterThanOrEqual(10)
  })

  it.each(fixture.cases.map((c) => [c.what, c] as const))(
    "agrees with the terminal UI about %s",
    (_what, c) => {
      expect(merged(build(c.sentence, c.args))).toEqual(merged(c.segments))
    },
  )
})
