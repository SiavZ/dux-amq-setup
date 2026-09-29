import { describe, expect, it } from "vitest"

import { changedPathsFrom, dirsToRefetch } from "./fileTreeFreshness"
import type { ChangesSliceView } from "./editorBuffers"

const LOADED = new Set(["", "src", "src/app"])
const LOADED_WITH_DOCS = new Set(["", "docs", "src"])

function row(path: string, status = "??", renamedFrom?: string) {
  return {
    path,
    status,
    additions: 0,
    deletions: 0,
    ...(renamedFrom ? { renamed_from: renamedFrom } : {}),
  }
}

function slice(
  unstaged: string[],
  staged: string[] = [],
): ChangesSliceView {
  return {
    phase: "loaded",
    unstaged: unstaged.map((p) => row(p)),
    staged: staged.map((p) => row(p)),
  }
}

describe("dirsToRefetch", () => {
  it("treats the first slice as a baseline, not a change", () => {
    expect(dirsToRefetch(null, ["src/new.ts"], LOADED)).toEqual([])
  })

  it("refetches the parent of an untracked file that appeared", () => {
    expect(dirsToRefetch([], ["src/app/new.ts"], LOADED)).toEqual(["src/app"])
  })

  it("refetches the root for an untracked directory git reports with a slash", () => {
    expect(dirsToRefetch([], ["newdir/"], LOADED)).toEqual([""])
  })

  it("refetches the root for a path that has no directory in it", () => {
    expect(dirsToRefetch([], ["NOTES.md"], LOADED)).toEqual([""])
  })

  it("refetches the nearest loaded ancestor of a file in a new folder", () => {
    expect(dirsToRefetch([], ["new-folder/nested/two.txt"], LOADED)).toEqual([
      "",
    ])
  })

  it("refetches a loaded folder for a file added straight into it", () => {
    expect(dirsToRefetch([], ["src/app/deep.ts"], LOADED)).toEqual(["src/app"])
  })

  it("asks for the shared ancestor once for two files in one new folder", () => {
    expect(dirsToRefetch([], ["src/new/a.ts", "src/new/b.ts"], LOADED)).toEqual(
      ["src"],
    )
  })

  it("asks for nothing when no ancestor of the path is loaded", () => {
    expect(dirsToRefetch([], ["docs/deep/new.ts"], new Set(["src"]))).toEqual(
      [],
    )
  })

  it("refetches the parent of a path that left the slice", () => {
    expect(dirsToRefetch(["src/gone.ts"], [], LOADED)).toEqual(["src"])
  })

  it("asks for nothing when the same paths are reported again", () => {
    const paths = ["src/a.ts", "src/app/b.ts"]
    expect(dirsToRefetch(paths, paths, LOADED)).toEqual([])
  })

  // A file moved between two loaded folders empties one listing and fills the
  // other, and only the destination is the record's own path.
  it("refetches both ends of a rename", () => {
    expect(
      dirsToRefetch([], ["docs/new.txt", "src/old.txt"], LOADED_WITH_DOCS),
    ).toEqual(["docs", "src"])
  })

  it("returns each affected directory once, sorted", () => {
    expect(
      dirsToRefetch(["src/gone.ts"], ["a.ts", "src/new.ts"], LOADED),
    ).toEqual(["", "src"])
  })
})

describe("changedPathsFrom", () => {
  it("is empty without a slice", () => {
    expect(changedPathsFrom(null)).toEqual([])
  })

  it("includes the path a rename came from", () => {
    const moved: ChangesSliceView = {
      phase: "loaded",
      unstaged: [],
      staged: [row("docs/new.txt", "R", "src/old.txt")],
    }
    expect(changedPathsFrom(moved)).toEqual(["docs/new.txt", "src/old.txt"])
  })

  it("unions the staged and unstaged paths, sorted and deduplicated", () => {
    expect(changedPathsFrom(slice(["b.ts", "a.ts"], ["a.ts", "c.ts"]))).toEqual([
      "a.ts",
      "b.ts",
      "c.ts",
    ])
  })
})
