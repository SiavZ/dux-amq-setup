import { isValidElement } from "react"
import { renderToStaticMarkup } from "react-dom/server"

import { type Prose, proseText } from "@/lib/prose"

// What a toast body says, whatever form it reached sonner in: a plain string
// stays itself, and a sentence whose names are drawn as chips is read back as
// the text a person sees (the chips' contents included, no markup). For tests
// that spy on sonner and assert on the words of a toast. Needs no DOM.

const ENTITIES: Record<string, string> = {
  "&amp;": "&",
  "&lt;": "<",
  "&gt;": ">",
  "&quot;": '"',
  "&#x27;": "'",
  "&#39;": "'",
}

function decode(html: string): string {
  return html.replace(/&(amp|lt|gt|quot|#x27|#39);/g, (e) => ENTITIES[e] ?? e)
}

export function toastText(body: unknown): string {
  if (typeof body === "string") return body
  // A sentence handed to a mocked `lib/notify.ts` before it became a node.
  if (Array.isArray(body)) return proseText(body as Prose)
  if (!isValidElement(body)) return String(body)
  return decode(renderToStaticMarkup(body).replace(/<[^>]*>/g, ""))
}

// Every name a toast body draws as a chip, in order.
export function toastChips(body: unknown): string[] {
  if (!isValidElement(body)) return []
  const html = renderToStaticMarkup(body)
  return [...html.matchAll(/<code data-slot="inline-code"[^>]*>(.*?)<\/code>/g)].map(
    (m) => decode(m[1]),
  )
}
