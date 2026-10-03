//! Centring measured lines with the odd column on the right.
//!
//! Ratatui's `Alignment::Center` halves the area width and the line width
//! separately (`area / 2 - line / 2`), so a line one column narrower than an
//! even area lands one cell right of centre, with the spare column on the
//! LEFT. Text that nearly fills a card shows it on every wrapped row. Every
//! centred surface in the app goes through this module instead, which splits
//! the slack once and leaves the odd column on the right, where a centred
//! button label already puts it.

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

/// The column a line `line_width` cells wide starts at when centred in `area`,
/// with the odd column of slack on the right. A line wider than the area
/// starts at its left edge.
pub(crate) fn centered_x(area: Rect, line_width: u16) -> u16 {
    area.x
        .saturating_add(area.width.saturating_sub(line_width) / 2)
}

/// Paint `lines` one per row from the top of `area`, each centred by
/// [`centered_x`] and cut at the area's right edge, over `base` (the style a
/// `Paragraph::style` would have painted the whole area with). Rows past the
/// area's height are dropped, as a `Paragraph` drops them. Callers wrap first
/// (see [`super::wrap_styled_lines`]); a line is never wrapped here, and a
/// line's own `alignment` is not consulted: every row is centred.
pub(crate) fn render_centered_lines(buf: &mut Buffer, area: Rect, lines: &[Line<'_>], base: Style) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    buf.set_style(area, base);
    for (row, line) in lines.iter().take(usize::from(area.height)).enumerate() {
        let width = u16::try_from(line.width())
            .unwrap_or(u16::MAX)
            .min(area.width);
        let x = centered_x(area, width);
        let y = area
            .y
            .saturating_add(u16::try_from(row).unwrap_or(u16::MAX));
        Paragraph::new(line.clone())
            .alignment(Alignment::Left)
            .style(base)
            .render(Rect::new(x, y, width, 1), buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    /// The case ratatui gets wrong: a 37-column line in a 38-column area takes
    /// offset 0 here and offset 1 from `Alignment::Center`.
    #[test]
    fn an_odd_column_of_slack_falls_on_the_right() {
        let area = Rect::new(0, 0, 38, 1);
        assert_eq!(centered_x(area, 37), 0);
        assert_eq!(centered_x(area, 36), 1);
        assert_eq!(centered_x(area, 35), 1);
        assert_eq!(centered_x(Rect::new(5, 0, 10, 1), 4), 8);

        let mut buf = Buffer::empty(area);
        let line = Line::from("x".repeat(37));
        render_centered_lines(&mut buf, area, &[line], Style::default());
        let painted = row(&buf, 0);
        assert!(
            painted.starts_with('x'),
            "no spare column on the left: {painted:?}"
        );
        assert!(
            painted.ends_with(' '),
            "the spare column is on the right: {painted:?}"
        );
    }

    /// Rows past the area's height are dropped and a line wider than the area
    /// is cut at its right edge, as a `Paragraph` does both.
    #[test]
    fn rows_are_bounded_by_the_area() {
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        let lines = [
            Line::from("abcdef"),
            Line::from("gh"),
            Line::from("dropped"),
        ];
        render_centered_lines(&mut buf, area, &lines, Style::default());
        assert_eq!(row(&buf, 0), "abcd");
        assert_eq!(row(&buf, 1), " gh ");
    }

    /// The base style fills the whole area, the padding included, and a span's
    /// own style patches over it.
    #[test]
    fn the_base_style_covers_the_padding_and_spans_patch_it() {
        use ratatui::style::Color;
        let area = Rect::new(0, 0, 6, 1);
        let mut buf = Buffer::empty(area);
        let base = Style::default().fg(Color::Red);
        let line = Line::from(vec![
            Span::raw("a"),
            Span::styled("b", Style::default().fg(Color::Blue)),
        ]);
        render_centered_lines(&mut buf, area, &[line], base);
        assert_eq!(row(&buf, 0), "  ab  ");
        assert_eq!(buf[(0u16, 0u16)].fg, Color::Red, "padding carries the base");
        assert_eq!(
            buf[(2u16, 0u16)].fg,
            Color::Red,
            "an unstyled span keeps the base"
        );
        assert_eq!(
            buf[(3u16, 0u16)].fg,
            Color::Blue,
            "a styled span patches it"
        );
    }
}
