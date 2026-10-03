import { describe, expect, it } from "vitest"
import { readBrandedStorage, removeBrandedStorage } from "./brandedStorage"

function memory() {
  const values = new Map<string, string>()
  const store = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value) },
    removeItem: (key: string) => { values.delete(key) },
  } as Storage
  return store
}

const keys = [
  ["yaran-chunk-reload", "dux-chunk-reload"],
  ["yaran:sidebar-width", "dux:sidebar-width"],
  ["yaran:changes-pane-percent", "dux:changes-pane-percent"],
  ["yaran-editor-explorer", "dux-editor-explorer"],
  ["yaran:typing-surface", "dux:typing-surface"],
  ["yaran:direct-input-hint", "dux:direct-input-hint"],
  ["yaran:theater-pill-position", "dux:theater-pill-position"],
  ["yaran:theater-pill-hint", "dux:theater-pill-hint"],
  ["yaran:theater:agent:tab-1", "dux:theater:agent:tab-1"],
  ["yaran:theater:terminal:term-1", "dux:theater:terminal:term-1"],
]

describe("browser branding compatibility", () => {
  it.each(keys)("promotes %s from %s", (key, old) => {
    const store = memory()
    store.setItem(old, "remembered")
    expect(readBrandedStorage(store, key)).toBe("remembered")
    expect(store.getItem(key)).toBe("remembered")
    expect(store.getItem(old)).toBe("remembered")
  })

  it("prefers current state, even an empty value", () => {
    const store = memory()
    store.setItem("dux:typing-surface", "compose")
    store.setItem("yaran:typing-surface", "")
    expect(readBrandedStorage(store, "yaran:typing-surface")).toBe("")
  })

  it("reset clears both names without resurrecting a legacy value", () => {
    const store = memory()
    store.setItem("dux:theater:agent:one", "on")
    store.setItem("yaran:theater:agent:one", "on")
    removeBrandedStorage(store, "yaran:theater:agent:one")
    expect(readBrandedStorage(store, "yaran:theater:agent:one")).toBeNull()
  })

  it("still reads legacy state when promotion exceeds quota", () => {
    const store = memory()
    store.setItem("dux:sidebar-width", "25rem")
    store.setItem = () => { throw new Error("quota") }
    expect(readBrandedStorage(store, "yaran:sidebar-width")).toBe("25rem")
    expect(store.getItem("dux:sidebar-width")).toBe("25rem")
    expect(store.getItem("yaran:sidebar-width")).toBeNull()
  })

  it("does not interpret unrelated keys as branding", () => {
    const store = memory()
    store.setItem("sidebar_state", "false")
    expect(readBrandedStorage(store, "sidebar_state")).toBe("false")
  })

  it("preserves caller-controlled error handling for blocked reads", () => {
    const store = memory()
    store.getItem = () => { throw new Error("blocked") }
    expect(() => readBrandedStorage(store, "yaran:typing-surface")).toThrow("blocked")
  })
})
