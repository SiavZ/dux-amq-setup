import type { ReactNode } from "react"

import { InlineCode } from "@/components/ui/inline-code"

// A sentence that names things, kept as structure rather than as a finished
// string, so the web can draw each name as a chip without hunting for it in the
// rendered words.
//
// Some sentences are also the terminal UI's, byte for byte, and there a name may
// be wrapped in straight quotes. `quotedChip` records that, so the plain-text
// form (`proseText`) reproduces the terminal's string exactly while the web form
// (`renderProse`) drops the quotes in favour of the chip. One wording, two
// spellings, and no way for them to disagree about the words.

export interface ProseName {
  name: string
  // The plain-text spelling wraps this name in straight double quotes.
  quoted: boolean
}

export type ProseSegment = string | ProseName

export type Prose = readonly ProseSegment[]

/** A name that the plain-text spelling leaves bare. */
export function chip(name: string): ProseName {
  return { name, quoted: false }
}

/** A name that the plain-text spelling wraps in straight double quotes. */
export function quotedChip(name: string): ProseName {
  return { name, quoted: true }
}

/** The plain-text spelling: what the terminal UI says, word for word. */
export function proseText(prose: Prose): string {
  return prose
    .map((segment) =>
      typeof segment === "string"
        ? segment
        : segment.quoted
          ? `"${segment.name}"`
          : segment.name,
    )
    .join("")
}

function isProseSegment(value: unknown): value is ProseSegment {
  if (typeof value === "string") return true
  if (typeof value !== "object" || value === null) return false
  const candidate = value as { name?: unknown; quoted?: unknown }
  return typeof candidate.name === "string" && typeof candidate.quoted === "boolean"
}

/**
 * Read an engine status off the wire: its parts when the server sent them and
 * they spell the message exactly, the plain message otherwise.
 *
 * The server builds both from the same parts (`dux_core::status_text`), so a
 * disagreement means something upstream is wrong, and the plain message is the
 * one the terminal UI printed. Nothing here ever looks for a name inside the
 * words: a sentence without parts stays text.
 */
export function wireProse(message: string, segments: unknown): string | Prose {
  if (!Array.isArray(segments) || !segments.every(isProseSegment)) return message
  return proseText(segments) === message ? segments : message
}

/** The web spelling: every name drawn through the shared inline code chip. */
export function renderProse(prose: Prose): ReactNode[] {
  return prose.map((segment, index) =>
    typeof segment === "string" ? (
      segment
    ) : (
      <InlineCode key={index}>{segment.name}</InlineCode>
    ),
  )
}
