import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

import { stripBidiControls } from "./bidi"

// Written as escapes: a literal control in a source file reorders how an
// editor or a code review draws the line it sits on, which is the very attack
// this module exists to defuse.
const CONTROLS = [
  "\u202A",
  "\u202B",
  "\u202C",
  "\u202D",
  "\u202E",
  "\u2066",
  "\u2067",
  "\u2068",
  "\u2069",
  "\u200E",
  "\u200F",
  "\u061C",
]

describe("stripBidiControls", () => {
  it("removes every embedding, override, isolate and directional mark", () => {
    for (const control of CONTROLS) {
      expect(stripBidiControls(`a${control}b`)).toBe("ab")
    }
  })

  it("leaves an override-crafted title in its stored order", () => {
    expect(stripBidiControls("Fix login \u202Eexe.txt\u202C")).toBe(
      "Fix login exe.txt",
    )
  })

  it("keeps right-to-left text and the joiner emoji need", () => {
    expect(stripBidiControls("תיקון באג")).toBe("תיקון באג")
    expect(stripBidiControls("a\u200Db")).toBe("a\u200Db")
  })

  it("is spelled without a single literal control in its own source", () => {
    const here = dirname(fileURLToPath(import.meta.url))
    for (const file of ["bidi.ts", "bidi.test.ts"]) {
      const source = readFileSync(join(here, file), "utf8")
      const literal = CONTROLS.filter((control) => source.includes(control))
      expect(literal, file).toEqual([])
    }
  })
})
