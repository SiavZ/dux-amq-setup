//! Snapshot-style tests for the scrollbar geometry that backs the agent
//! pane in `src/app/render.rs`. We render through ratatui's `TestBackend`
//! and assert the symbol at specific cells so we don't drift if the
//! underlying `ScrollbarState` math changes between ratatui releases.
//!
//! Audit reference: docs/plans/audits/audit01/10-scrollbar-math-and-tests.md
//! (P1-4, P2-3). The earlier `ScrollbarState::new(total + visible)
//! .viewport_content_length(visible)` form inflated the denominator so
//! the thumb could never reach the bottom; this file pins the
//! `ScrollbarState::new(total).position(total - offset - 1)` form.
//!
//! All defaults shown in `ratatui-widgets-0.3.0` for a `VerticalRight`
//! scrollbar:
//! - begin symbol: "▲"
//! - end   symbol: "▼"
//! - track symbol: "║" (DOUBLE_VERTICAL)
//! - thumb symbol: "█" (FULL block)

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};

const BEGIN: &str = "▲";
const END: &str = "▼";
const TRACK: &str = "║";
const THUMB: &str = "█";

/// Mirror of the production formula in `src/app/render.rs`. Kept inline so
/// the test file is self-contained and doesn't have to expose the helper
/// from the `dux` crate just for testing.
fn position_for(total: usize, offset: usize) -> usize {
    let offset = offset.min(total);
    total.saturating_sub(offset).saturating_sub(1)
}

fn render_scrollbar(width: u16, height: u16, total: usize, offset: usize) -> Terminal<TestBackend> {
    let mut term =
        Terminal::new(TestBackend::new(width, height)).expect("TestBackend init must succeed");
    let position = position_for(total, offset);
    let mut state = ScrollbarState::new(total).position(position);
    term.draw(|frame| {
        let area = frame.area();
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut state,
        );
    })
    .expect("draw must succeed");
    term
}

fn column_symbols(term: &Terminal<TestBackend>, col: u16) -> Vec<String> {
    let buf = term.backend().buffer();
    let height = buf.area.height;
    (0..height)
        .map(|row| buf.cell((col, row)).unwrap().symbol().to_string())
        .collect()
}

#[test]
fn thumb_is_at_bottom_when_offset_is_zero() {
    // total=100, visible=10, offset=0 ⇒ user is at the latest line.
    // ratatui begin/end arrows take the first/last row, so thumb must
    // appear in the last interior cell (row 8 here, below the track).
    let term = render_scrollbar(20, 10, 100, 0);
    let col = column_symbols(&term, 19);
    assert_eq!(col[0], BEGIN, "first row is the begin arrow");
    assert_eq!(col[9], END, "last row is the end arrow");
    assert_eq!(
        col[8], THUMB,
        "thumb should sit just above the bottom arrow"
    );
    // Nothing else in the track should be the thumb.
    for (i, sym) in col.iter().enumerate().skip(1).take(7) {
        assert_eq!(sym, TRACK, "row {i} should still be track, got {sym:?}");
    }
}

#[test]
fn thumb_is_at_top_when_offset_is_total_minus_one() {
    // offset = total - 1 ⇒ user paged all the way to the oldest line.
    let term = render_scrollbar(20, 10, 100, 99);
    let col = column_symbols(&term, 19);
    assert_eq!(col[0], BEGIN);
    assert_eq!(col[9], END);
    assert_eq!(col[1], THUMB, "thumb should sit just below the top arrow");
    for (i, sym) in col.iter().enumerate().skip(2).take(7) {
        assert_eq!(sym, TRACK, "row {i} should still be track, got {sym:?}");
    }
}

#[test]
fn thumb_is_in_the_middle_when_offset_is_half() {
    // offset = total/2 ⇒ thumb should be in the middle of the track.
    // For total=100 with track_length=8 the thumb falls roughly mid-
    // track; we tolerate a row of slop because ratatui rounds to the
    // nearest integer when computing the thumb start.
    let term = render_scrollbar(20, 10, 100, 50);
    let col = column_symbols(&term, 19);
    assert_eq!(col[0], BEGIN);
    assert_eq!(col[9], END);
    let thumb_row = col
        .iter()
        .position(|s| s == THUMB)
        .expect("thumb must be present somewhere in the track");
    assert!(
        (3..=6).contains(&thumb_row),
        "thumb should be roughly centered (rows 3..=6), got row {thumb_row}",
    );
}

#[test]
fn small_content_does_not_render_a_scrollbar_when_content_length_is_zero() {
    // The production guard in `src/app/render.rs` is `total > 0 &&
    // visible > 0 && term_area.width >= 2`. When `total == 0` we skip
    // rendering entirely. This test pins ratatui's matching short-
    // circuit (see ratatui-widgets-0.3.0 scrollbar.rs render(): the
    // first guard returns early if `state.content_length == 0`), so the
    // buffer stays untouched.
    let mut term = Terminal::new(TestBackend::new(20, 10)).expect("TestBackend init must succeed");
    let mut state = ScrollbarState::new(0);
    term.draw(|frame| {
        let area = frame.area();
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut state,
        );
    })
    .expect("draw must succeed");
    let col = column_symbols(&term, 19);
    // Empty cells render as a single space.
    for (i, sym) in col.iter().enumerate() {
        assert_eq!(
            sym, " ",
            "row {i} should be empty when content_length=0, got {sym:?}",
        );
    }
}

#[test]
fn position_for_clamps_offset_above_total() {
    // Defensive: an offset that exceeds total should still produce a
    // valid (top-of-track) position rather than panicking on overflow.
    assert_eq!(position_for(10, 999), 0);
    // total=0 should saturate to 0 too.
    assert_eq!(position_for(0, 0), 0);
    assert_eq!(position_for(0, 5), 0);
}
