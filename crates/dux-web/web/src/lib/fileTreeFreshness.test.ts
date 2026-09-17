import { describe, expect, it } from "vitest"

import { changedPathsFrom, dirsToRefetch } from "./fileTreeFreshness"
import type { ChangesSliceView } from "./editorBuffers"

const LOADED = new Set(["", "src", "src/app"])

function row(path: string, status = "??") {
  return { path, status, additions: 0, deletions: 0 }
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

  it("ignores a path whose directory is not loaded", () => {
    expect(dirsToRefetch([], ["docs/deep/new.ts"], LOADED)).toEqual([])
  })

  it("refetches the parent of a path that left the slice", () => {
    expect(dirsToRefetch(["src/gone.ts"], [], LOADED)).toEqual(["src"])
  })

  it("asks for nothing when the same paths are reported again", () => {
    const paths = ["src/a.ts", "src/app/b.ts"]
    expect(dirsToRefetch(paths, paths, LOADED)).toEqual([])
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

  it("unions the staged and unstaged paths, sorted and deduplicated", () => {
    expect(changedPathsFrom(slice(["b.ts", "a.ts"], ["a.ts", "c.ts"]))).toEqual([
      "a.ts",
      "b.ts",
      "c.ts",
    ])
  })
})
