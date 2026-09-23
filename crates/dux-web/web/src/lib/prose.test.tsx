// @vitest-environment jsdom
import { describe, expect, it } from "vitest"
import { render } from "@testing-library/react"

import { chip, proseText, quotedChip, renderProse } from "./prose"

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
