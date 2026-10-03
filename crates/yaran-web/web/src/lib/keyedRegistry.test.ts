// @vitest-environment jsdom
import { describe, expect, it } from "vitest"
import { renderHook } from "@testing-library/react"

import { createKeyedRegistry } from "./keyedRegistry"

// The shared half of the three pane registries. Each of them keeps its own tests
// for what it MEANS; what is pinned here is the mechanism they share.

describe("a keyed registry", () => {
  it("answers per key, and reads undefined for one nobody published", () => {
    const registry = createKeyedRegistry<string>()
    registry.register("a", "one")
    registry.register("b", "two")
    expect(registry.read("a")).toBe("one")
    expect(registry.read("b")).toBe("two")
    expect(registry.read("c")).toBeUndefined()
  })

  it("lets the last write win, and the retirement clear it", () => {
    const registry = createKeyedRegistry<string>()
    registry.register("a", "one")
    const retire = registry.register("a", "two")
    expect(registry.read("a")).toBe("two")
    retire()
    expect(registry.read("a")).toBeUndefined()
  })

  it("keeps a replacement's entry through the outgoing registration's cleanup", () => {
    // React does not order an old cleanup before a new effect, so a late
    // retirement must not clear what the successor has already written. Told
    // apart by identity, never by the value, which is why a caller publishing a
    // primitive boxes it.
    const registry = createKeyedRegistry<{ on: boolean }>()
    const stale = registry.register("a", { on: true })
    const live = { on: true }
    registry.register("a", live)
    stale()
    expect(registry.read("a")).toBe(live)
  })

  it("re-renders a subscriber on every change, including a retirement", () => {
    const registry = createKeyedRegistry<string>()
    const { result, rerender } = renderHook(() => {
      registry.useVersion()
      return registry.read("a")
    })
    expect(result.current).toBeUndefined()
    const retire = registry.register("a", "one")
    rerender()
    expect(result.current).toBe("one")
    retire()
    rerender()
    expect(result.current).toBeUndefined()
  })

  it("forgets everything on reset, and keeps registries apart", () => {
    const one = createKeyedRegistry<string>()
    const two = createKeyedRegistry<string>()
    one.register("a", "one")
    two.register("a", "two")
    one.reset()
    expect(one.read("a")).toBeUndefined()
    expect(two.read("a")).toBe("two")
  })
})
