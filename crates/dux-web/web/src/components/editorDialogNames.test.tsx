// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { ConfirmReloadFileDialog } from "./ConfirmReloadFileDialog"
import { DeleteEntryDialog } from "./DeleteEntryDialog"
import { NewEntryDialog } from "./NewEntryDialog"
import { RenameEntryDialog } from "./RenameEntryDialog"
import { SaveConflictDialog } from "./SaveConflictDialog"

// Every path an editor dialog names, in its title and in its body, is the
// shared chip rather than an ad hoc monospace span.
afterEach(cleanup)

const noop = () => {}

function chipsIn(el: Element | null): (string | null)[] {
  return [...(el?.querySelectorAll("code") ?? [])].map((c) => c.textContent)
}

function isChip(el: Element | undefined) {
  expect(el?.tagName).toBe("CODE")
  expect(el?.getAttribute("data-slot")).toBe("inline-code")
}

describe("editor dialogs name paths as chips", () => {
  it("Delete names the path in the title and the body", () => {
    render(
      <DeleteEntryDialog
        target={{ path: "src/a.ts", isDir: false }}
        onClose={noop}
        onConfirm={noop}
      />,
    )
    const [inTitle, inBody] = screen.getAllByText("src/a.ts")
    isChip(inTitle)
    isChip(inBody)
    expect(inTitle.closest("h2")?.textContent).toBe("Delete src/a.ts?")
  })

  it("Delete names the path while a save blocks it", () => {
    render(
      <DeleteEntryDialog
        target={{ path: "src/a.ts", isDir: false }}
        blockedBySave
        onClose={noop}
        onConfirm={noop}
      />,
    )
    isChip(screen.getByText(/currently being/).querySelector("code") ?? undefined)
  })

  it("the save conflict names the path, changed or deleted", () => {
    for (const deleted of [false, true]) {
      render(
        <SaveConflictDialog
          target={{ tabId: "t", path: "src/a.ts", body: "x", deleted } as never}
          onOverwrite={noop}
          onReload={noop}
          onClose={noop}
        />,
      )
      isChip(screen.getByText("src/a.ts"))
      cleanup()
    }
  })

  it("the reload confirm names the path", () => {
    render(
      <ConfirmReloadFileDialog
        target={{ tabId: "t", path: "src/a.ts" }}
        present
        onClose={noop}
        onConfirm={noop}
      />,
    )
    isChip(screen.getByText("src/a.ts"))
  })

  it("New file names the folder it lands in", () => {
    render(
      <NewEntryDialog
        target={{ kind: "file", dir: "src/lib" }}
        onClose={noop}
        onSubmit={() => Promise.resolve()}
      />,
    )
    isChip(screen.getByText("src/lib"))
  })

  it("Rename names the entry in the title", () => {
    render(
      <RenameEntryDialog
        target={{ path: "src/a/old.ts", isDir: false }}
        isDirty={false}
        onClose={noop}
        onSubmit={() => Promise.resolve()}
      />,
    )
    const title = screen.getByRole("heading")
    expect(title.textContent).toBe("Rename old.ts")
    expect(chipsIn(title)).toEqual(["old.ts"])
  })
})
