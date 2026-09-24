// Removing the Unicode controls that reorder how text is displayed from text
// dux did not write.
//
// A pull request title is written by whoever opened the pull request. The
// embedding, override and isolate controls (U+202A to U+202E, U+2066 to U+2069)
// and the directional marks (U+200E, U+200F, U+061C) make the browser draw
// characters in an order other than the one they are stored in, so a crafted
// title can make the words around it read as something they do not say. The
// server strips them where the title enters (dux-core's `bidi.rs`); the web
// strips them again where the workspace enters its store, so a screen drawing
// the title never has to remember to.

const BIDI_CONTROLS = /[‪-‮⁦-⁩‎‏؜]/g

/** `text` with every bidirectional control removed. */
export function stripBidiControls(text: string): string {
  return text.replace(BIDI_CONTROLS, "")
}
