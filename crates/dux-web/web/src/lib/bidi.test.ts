import { describe, expect, it } from "vitest"

import { stripBidiControls } from "./bidi"

describe("stripBidiControls", () => {
  it("removes every embedding, override, isolate and directional mark", () => {
    const controls = [
      "‪",
      "‫",
      "‬",
      "‭",
      "‮",
      "⁦",
      "⁧",
      "⁨",
      "⁩",
      "‎",
      "‏",
      "؜",
    ]
    for (const control of controls) {
      expect(stripBidiControls(`a${control}b`)).toBe("ab")
    }
  })

  it("leaves an override-crafted title in its stored order", () => {
    expect(stripBidiControls("Fix login ‮exe.txt‬")).toBe("Fix login exe.txt")
  })

  it("keeps right-to-left text and the joiner emoji need", () => {
    expect(stripBidiControls("תיקון באג")).toBe("תיקון באג")
    expect(stripBidiControls("a‍b")).toBe("a‍b")
  })
})
