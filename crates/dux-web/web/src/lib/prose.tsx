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
