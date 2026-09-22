import { rootApiBase, type EditorRoot } from "@/lib/editorRoot"

// Recognize markdown files by extension so the editor can offer a rendered
// preview toggle only where it makes sense. Case-insensitive.
const MARKDOWN_EXTENSIONS = [".md", ".markdown", ".mdown", ".mkd", ".mdx"]

export function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase()
  return MARKDOWN_EXTENSIONS.some((ext) => lower.endsWith(ext))
}

// The fragment an `#…` href names, percent-decoding included. A malformed
// escape decodes to nothing, so the raw text stands in rather than throwing.
export function decodeFragment(href: string): string {
  const raw = href.slice(1)
  try {
    return decodeURIComponent(raw)
  } catch {
    return raw
  }
}

// GitHub's heading-slug rule, which is what a hand-written `[x](#section)` in a
// README was written against. Letters, digits, marks and `_` survive; other
// punctuation is dropped and spaces become hyphens.
export function headingSlug(text: string): string {
  return text
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\p{M}_ -]/gu, "")
    .replace(/ /g, "-")
}

const HEADINGS = "h1, h2, h3, h4, h5, h6"

// The element in the rendered preview a fragment names, or null when the
// document has nothing by that name. Three answers are tried because the
// preview's ids come from three places: an id react-markdown passed through,
// an id the sanitizer clobber-prefixed out of author HTML, and no id at all,
// which is every heading, matched by slugged text the way GitHub numbers
// repeats.
export function previewFragmentTarget(
  container: HTMLElement,
  fragment: string,
): HTMLElement | null {
  if (!fragment) return null
  // Compared rather than selected: a fragment is arbitrary author text, and
  // building a selector out of it would need escaping to stay well formed.
  const clobbered = `user-content-${fragment}`
  for (const el of container.querySelectorAll<HTMLElement>("[id]")) {
    if (el.id === fragment || el.id === clobbered) return el
  }
  const seen = new Map<string, number>()
  for (const heading of container.querySelectorAll<HTMLElement>(HEADINGS)) {
    const slug = headingSlug(heading.textContent ?? "")
    if (!slug) continue
    const nth = seen.get(slug) ?? 0
    seen.set(slug, nth + 1)
    if ((nth === 0 ? slug : `${slug}-${nth}`) === fragment) return heading
  }
  return null
}

// A URL is external, and left alone in the preview, when it carries a scheme, is
// protocol-relative, or is root-absolute: anything but a worktree-relative path.
function isExternalUrl(url: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(url) || url.startsWith("//") || url.startsWith("/")
}

// Resolve a relative asset reference against the markdown file's own directory into a
// normalized, worktree-relative path. Null when the reference is external or escapes the
// worktree root via `..`, and the caller then leaves the URL untouched. Query and hash dropped.
export function resolveWorktreeRelative(
  filePath: string,
  src: string,
): string | null {
  if (!src || isExternalUrl(src)) return null
  const bare = src.split(/[?#]/, 1)[0]
  if (!bare) return null
  const slash = filePath.lastIndexOf("/")
  const baseParts = slash === -1 ? [] : filePath.slice(0, slash).split("/")
  const stack: string[] = [...baseParts]
  for (const part of bare.split("/")) {
    if (part === "" || part === ".") continue
    if (part === "..") {
      if (stack.length === 0) return null // escapes the worktree root
      stack.pop()
    } else {
      stack.push(part)
    }
  }
  return stack.length > 0 ? stack.join("/") : null
}

// The same-origin proxy URL serving an asset under the editor's root, or null when `src` is
// not a root-relative reference. The server re-validates the path for containment in that root.
export function markdownAssetUrl(
  root: EditorRoot,
  filePath: string,
  src: string,
): string | null {
  const rel = resolveWorktreeRelative(filePath, src)
  if (rel === null) return null
  return `${rootApiBase(root)}/files/raw?path=${encodeURIComponent(rel)}`
}
