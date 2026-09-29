// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"
import { agentRoot, type EditorRoot } from "@/lib/editorRoot"

const { default: MarkdownPreview } = await import("./MarkdownPreview")

afterEach(cleanup)

const root: EditorRoot = agentRoot("s1")

function preview(content: string) {
  return render(<MarkdownPreview content={content} root={root} path="a.md" />)
}

// Each row of the front-matter table as [key, value].
function tableRows(): Array<[string, string]> {
  const rows = Array.from(document.querySelectorAll("tbody tr"))
  return rows.map((row) => {
    const cells = Array.from(row.querySelectorAll("th, td"))
    return [cells[0]?.textContent ?? "", cells[1]?.textContent ?? ""]
  })
}

describe("MarkdownPreview front matter", () => {
  it("renders the leading YAML block as a key/value table above the body", () => {
    preview(
      [
        "---",
        "title: Hello world",
        "draft: true",
        "tags: [rust, web]",
        "---",
        "",
        "# Body",
        "",
        "text",
      ].join("\n"),
    )
    const table = screen.getByRole("table")
    expect(table).toBeTruthy()
    expect(tableRows()).toEqual([
      ["title", "Hello world"],
      ["draft", "true"],
      ["tags", "rust, web"],
    ])
    // The table precedes the prose it describes.
    const heading = screen.getByRole("heading", { name: "Body" })
    expect(table.compareDocumentPosition(heading)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    )
  })

  it("renders the body without the front-matter block", () => {
    const { container } = preview("---\ntitle: x\n---\n\n# Body\n\ntext\n")
    expect(screen.getByRole("heading", { name: "Body" })).toBeTruthy()
    expect(container.textContent).not.toContain("---")
    expect(container.querySelector("hr")).toBeNull()
  })

  it("leaves a document without front matter unchanged", () => {
    const { container } = preview("# Body\n\ntext with a: colon\n")
    expect(container.querySelector("table")).toBeNull()
    expect(container.textContent).toContain("text with a: colon")
  })

  it("renders no table for an empty block but still drops it", () => {
    const { container } = preview("---\n---\n\n# Body\n")
    expect(container.querySelector("table")).toBeNull()
    expect(screen.getByRole("heading", { name: "Body" })).toBeTruthy()
  })

  it("shows an unreadable block as its own raw text rather than dropping it", () => {
    const { container } = preview("---\n- a\n- b\n---\n\n# Body\n")
    expect(container.querySelector("table")).toBeNull()
    const pre = container.querySelector("pre")
    expect(pre?.textContent).toBe("- a\n- b")
    expect(screen.getByRole("heading", { name: "Body" })).toBeTruthy()
  })

  it("strips only the first block, leaving a second one to render as markdown", () => {
    const { container } = preview("---\nk: v\n---\n---\nx: y\n---\nbody\n")
    expect(tableRows()).toEqual([["k", "v"]])
    expect(container.querySelector("hr")).toBeTruthy()
    expect(screen.getByRole("heading", { name: "x: y" })).toBeTruthy()
    expect(container.textContent).toContain("body")
  })

  it("lets a long unbroken value wrap instead of widening the table", () => {
    preview(`---\nurl: ${"a".repeat(200)}\n---\n\nbody\n`)
    const cell = document.querySelector("tbody td")
    expect(cell?.className).toContain("break-words")
  })

  it("escapes markup in a front-matter value instead of rendering it", () => {
    const { container } = preview(
      '---\ntitle: "<img src=x onerror=alert(1)> **bold**"\n---\n\nbody\n',
    )
    expect(container.querySelector("img")).toBeNull()
    expect(container.querySelector("strong")).toBeNull()
    expect(tableRows()).toEqual([
      ["title", "<img src=x onerror=alert(1)> **bold**"],
    ])
  })
})

describe("MarkdownPreview in-document anchors", () => {
  let scrolled: Element[]
  let hashChanges: number

  function countHashChange() {
    hashChanges += 1
  }

  beforeEach(() => {
    scrolled = []
    hashChanges = 0
    Element.prototype.scrollIntoView = function scrollIntoView(this: Element) {
      scrolled.push(this)
    }
    window.addEventListener("hashchange", countHashChange)
  })

  afterEach(() => {
    window.removeEventListener("hashchange", countHashChange)
    window.location.hash = ""
  })

  // A real click, so the anchor's own default action is what gets cancelled.
  // Keyboard activation arrives here too: Enter on a focused link dispatches a
  // click, which is why the handler needs no key listener of its own.
  function clickLink(container: HTMLElement, selector: string): MouseEvent {
    const link = container.querySelector(selector)
    expect(link).toBeTruthy()
    const event = new MouseEvent("click", { bubbles: true, cancelable: true })
    link?.dispatchEvent(event)
    return event
  }

  it("scrolls to the heading a fragment link names and leaves the URL alone", () => {
    window.location.hash = "#/agent/s1/editor/file/notes.md"
    const { container } = preview("## My Heading\n\n[jump](#my-heading)\n")
    const event = clickLink(container, 'a[href="#my-heading"]')
    // jsdom performs no fragment navigation of its own, so cancelling the
    // event is the load-bearing assertion: uncancelled, the browser would
    // write `#my-heading` over the app's whole address.
    expect(event.defaultPrevented).toBe(true)
    expect(hashChanges).toBe(0)
    expect(window.location.hash).toBe("#/agent/s1/editor/file/notes.md")
    expect(scrolled).toEqual([container.querySelector("h2")])
  })

  it("resolves an id the sanitizer clobber-prefixed", () => {
    const { container } = preview('<h3 id="manual">M</h3>\n\n[go](#manual)\n')
    const event = clickLink(container, 'a[href="#manual"]')
    expect(event.defaultPrevented).toBe(true)
    expect(scrolled).toEqual([container.querySelector("h3")])
  })

  it("lands on an author anchor that shares its id with a heading slug", () => {
    // GitHub's renderer has the same collision: an author anchor keeps its
    // id and the heading gets the same slug, so two elements share one id.
    // The first in document order wins the fragment, which is the anchor the
    // author placed on purpose; that is accepted rather than de-duplicated.
    const { container } = preview(
      '<a id="notes"></a>\n\n## Notes\n\n[jump](#notes)\n',
    )
    const twins = container.querySelectorAll("#user-content-notes")
    expect(twins.length).toBe(2)
    clickLink(container, 'a[href="#notes"]')
    expect(scrolled).toEqual([container.querySelector('a[id="user-content-notes"]')])
  })

  it("decodes a percent-encoded fragment", () => {
    const { container } = preview("## Café\n\n[jump](#caf%C3%A9)\n")
    expect(container.querySelector("h2")?.id).toBe("user-content-café")
    clickLink(container, "a")
    expect(scrolled).toEqual([container.querySelector("h2")])
  })

  it("resolves a percent-encoded fragment naming a non-Latin heading", () => {
    const { container } = preview("## 日本語の見出し\n\n[jump](#%E6%97%A5%E6%9C%AC%E8%AA%9E%E3%81%AE%E8%A6%8B%E5%87%BA%E3%81%97)\n")
    expect(container.querySelector("h2")?.id).toBe("user-content-日本語の見出し")
    clickLink(container, "a")
    expect(scrolled).toEqual([container.querySelector("h2")])
  })

  it("slugs punctuation the way GitHub does, runs of it included", () => {
    // `C++ & Rust: notes` keeps the gap each dropped symbol sat in, so the
    // written anchor carries two hyphens where the `&` was.
    const { container } = preview(
      "## C++ & Rust: notes\n\n[jump](#c--rust-notes)\n",
    )
    expect(container.querySelector("h2")?.id).toBe("user-content-c--rust-notes")
    clickLink(container, 'a[href="#c--rust-notes"]')
    expect(scrolled).toEqual([container.querySelector("h2")])
  })

  it("numbers repeated headings the way a written anchor expects", () => {
    const { container } = preview("## Notes\n\n## Notes\n\n[second](#notes-1)\n")
    const headings = container.querySelectorAll("h2")
    expect(headings[0]?.id).toBe("user-content-notes")
    expect(headings[1]?.id).toBe("user-content-notes-1")
    clickLink(container, 'a[href="#notes-1"]')
    expect(scrolled).toEqual([headings[1]])
  })

  it("numbers repeats from the first heading again in each render", () => {
    // The plugin holds one slugger across renders, so a repeat in the next
    // document must not be numbered as though it followed this one.
    const markdown = "## Notes\n\n## Notes\n\n[second](#notes-1)\n"
    preview(markdown)
    cleanup()
    const { container } = preview(markdown)
    expect(container.querySelectorAll("h2")[1]?.id).toBe("user-content-notes-1")
  })

  it("does nothing at all when the anchor names no target", () => {
    window.location.hash = "#/agent/s1/editor/file/notes.md"
    const { container } = preview("[nowhere](#missing)\n")
    const event = clickLink(container, 'a[href="#missing"]')
    expect(event.defaultPrevented).toBe(true)
    expect(scrolled).toEqual([])
    expect(hashChanges).toBe(0)
    expect(window.location.hash).toBe("#/agent/s1/editor/file/notes.md")
  })

  it("still sends an external link to a new tab", () => {
    const open = vi.spyOn(window, "open").mockImplementation(() => null)
    const { container } = preview("[out](https://example.com/x)\n")
    const event = clickLink(container, 'a[href="https://example.com/x"]')
    expect(event.defaultPrevented).toBe(true)
    expect(open).toHaveBeenCalledWith(
      "https://example.com/x",
      "_blank",
      "noopener,noreferrer",
    )
    open.mockRestore()
  })
})
