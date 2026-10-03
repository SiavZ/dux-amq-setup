import { describe, expect, it } from "vitest"

import {
  DEFAULT_INSTANCE_TITLE,
  pageTitle,
  resolveInstanceTitle,
} from "./instanceTitle"

describe("pageTitle", () => {
  it("prefixes the standalone editor tab so it reads as its own surface", () => {
    expect(pageTitle("yaran @ devbox", true)).toBe("Editor: yaran @ devbox")
  })

  it("leaves the workspace tab's title as the instance title alone", () => {
    expect(pageTitle("yaran @ devbox", false)).toBe("yaran @ devbox")
  })
})

describe("resolveInstanceTitle", () => {
  it("returns a configured title verbatim", () => {
    expect(resolveInstanceTitle("yaran #1")).toBe("yaran #1")
  })

  it("trims surrounding whitespace", () => {
    expect(resolveInstanceTitle("  yaran (prod)  ")).toBe("yaran (prod)")
  })

  it("falls back to the product name when missing", () => {
    expect(resolveInstanceTitle(undefined)).toBe(DEFAULT_INSTANCE_TITLE)
    expect(resolveInstanceTitle(null)).toBe(DEFAULT_INSTANCE_TITLE)
  })

  it("falls back when empty or whitespace only", () => {
    expect(resolveInstanceTitle("")).toBe("yaran")
    expect(resolveInstanceTitle("   ")).toBe("yaran")
  })

  it("collapses internal newlines so the tab and wordmark stay identical", () => {
    // A hand-edited config can contain a TOML newline escape; browsers truncate a
    // tab title at the first newline while a nowrap span would show a space.
    expect(resolveInstanceTitle("yaran\nlab")).toBe("yaran lab")
  })

  it("collapses internal tabs and mixed control-whitespace runs to one space", () => {
    expect(resolveInstanceTitle("yaran\tlab")).toBe("yaran lab")
    expect(resolveInstanceTitle("yaran\r\n\tlab")).toBe("yaran lab")
  })
})
