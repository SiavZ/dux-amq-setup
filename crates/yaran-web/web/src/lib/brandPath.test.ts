import { readFileSync } from "node:fs"
import { describe, expect, it } from "vitest"
import { BRAND_PATH, brandFaviconDataUri } from "./favicon"

const canonical = readFileSync(new URL("../../../../../assets/yaran-logo.svg", import.meta.url), "utf8")

describe("canonical Yaran mark", () => {
  it("uses the canonical vector for tinted favicons", () => {
    expect(canonical).toContain(`d="${BRAND_PATH}"`)
    const favicon = decodeURIComponent(brandFaviconDataUri("#863bff"))
    expect(favicon).toContain(`d="${BRAND_PATH}"`)
  })

  it("has a closed, nonempty path without attribute breakout characters", () => {
    expect(BRAND_PATH).toMatch(/^M[MLHVZmlhvz0-9.,\s-]+Z$/)
    expect(BRAND_PATH).not.toContain('"')
    expect(BRAND_PATH).not.toContain("<")
  })

  it("labels the canonical artwork as Yaran, not the upstream duck", () => {
    expect(canonical).toContain(">Yaran</title>")
    expect(canonical).not.toMatch(/duck|dux/i)
    expect(canonical).toContain('viewBox="0 0 512 512"')
  })
})
