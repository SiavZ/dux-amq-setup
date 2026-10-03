// The text spinner's frames and slot class, in a module with no React import
// so the font-asset tests (terminalFonts.test.ts) can read the frames without
// mounting a component. The component is components/GlyphSpinner.tsx.

/// The six arc frames (U+25DC-U+25E1), matching the yaran TUI's `SPINNER_FRAMES`
/// (crates/yaran-tui/src/theme.rs) and yaran-core's status-line spinner, so the
/// two surfaces show the same spinner.
export const SPINNER_FRAMES = ["◜", "◠", "◝", "◞", "◡", "◟"]

/// The cadence of yaran-core's status-line spinner. The TUI's own row spinner
/// runs faster (`SPINNER_FRAME_MS` in crates/yaran-tui/src/theme.rs).
export const SPINNER_FRAME_MS = 100

/// The class carrying the fixed-width slot and the font stack that actually
/// has these glyphs. Defined in index.css; the comment there says why the slot
/// is fixed and why that face is named.
export const GLYPH_SPINNER_CLASS = "glyph-spinner"
