//! The name chip: how a variable inside a dialog's prose looks.
//!
//! Every branch, path, file, command, and agent, project, terminal or provider
//! name a dialog sentence carries renders as one span in the theme's
//! `name_fg` on `name_bg`, padded by one no-break cell on each side, with no bold and no
//! quotes: the chip is the delimiter. A padded chip is exactly as wide as the
//! quoted text it replaces, so a sentence that used to quote its names keeps
//! its width. It is the terminal counterpart of the web's inline code chip.
//!
//! Sentences are built as [`Prose`] (constant words and names), including the
//! ones dux-core shares with the web, and turned into spans only here. A call
//! site never styles a name by hand.

use dux_core::prose::{Prose, ProseSegment};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::wrap_lines::NO_BREAK_SPACE;
use crate::theme::Theme;

/// One name as a chip.
///
/// The padding is the no-break space, so a wrapping body moves the chip to the
/// next row whole instead of breaking at a pad and starting the row with a
/// chip that has lost its left edge. ratatui's own wrapper and
/// [`super::wrap_styled_lines`] both refuse to break there. The cost is that a
/// selection copied out of the host terminal carries U+00A0 at the edges of a
/// name when it takes the padding with it.
pub(crate) fn name_chip(name: &str, theme: &Theme) -> Span<'static> {
    Span::styled(
        format!("{NO_BREAK_SPACE}{name}{NO_BREAK_SPACE}"),
        theme.name_style(),
    )
}

/// A sentence as spans: constant words in `text_style`, every name as a chip.
/// A name built quoted loses its quotes, because the chip replaces them.
pub(crate) fn prose_spans(prose: &Prose, text_style: Style, theme: &Theme) -> Vec<Span<'static>> {
    prose
        .segments()
        .iter()
        .map(|segment| match segment {
            ProseSegment::Text(text) => Span::styled(text.clone(), text_style),
            ProseSegment::Name { name, .. } => name_chip(name, theme),
        })
        .collect()
}

/// An info row that names one thing: `label` in `label_style`, then the name
/// as a chip (" Agent:  feat/login ").
pub(crate) fn labelled_name(
    label: &str,
    name: &str,
    label_style: Style,
    theme: &Theme,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(label.to_string(), label_style),
        name_chip(name, theme),
    ])
}

/// A sentence as lines, split at every line break in its words, each line
/// starting with `indent` in `text_style` (the dialogs' one-cell body margin).
pub(crate) fn prose_lines(
    prose: &Prose,
    indent: &str,
    text_style: Style,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let start = || vec![Span::styled(indent.to_string(), text_style)];
    let mut lines = Vec::new();
    let mut current = start();
    for segment in prose.segments() {
        match segment {
            ProseSegment::Text(text) => {
                for (index, piece) in text.split('\n').enumerate() {
                    if index > 0 {
                        lines.push(Line::from(std::mem::replace(&mut current, start())));
                    }
                    if !piece.is_empty() {
                        current.push(Span::styled(piece.to_string(), text_style));
                    }
                }
            }
            ProseSegment::Name { name, .. } => current.push(name_chip(name, theme)),
        }
    }
    lines.push(Line::from(current));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::{Color, Modifier};

    fn theme() -> Theme {
        let mut theme = Theme::default_dark();
        theme.name_fg = Color::Rgb(1, 2, 3);
        theme.name_bg = Color::Rgb(4, 5, 6);
        theme
    }

    #[test]
    fn a_name_is_padded_by_one_cell_in_the_chip_colors_and_never_bold() {
        let chip = name_chip("feat/login", &theme());
        assert_eq!(chip.content, "\u{a0}feat/login\u{a0}");
        assert_eq!(chip.style.fg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(chip.style.bg, Some(Color::Rgb(4, 5, 6)));
        assert!(!chip.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn a_padded_chip_is_as_wide_as_the_quoted_name_it_replaces() {
        for name in ["main", "feat/ünïcode", "日本語"] {
            let quoted = Span::raw(format!("\"{name}\""));
            assert_eq!(name_chip(name, &theme()).width(), quoted.width(), "{name}");
        }
    }

    #[test]
    fn prose_becomes_words_in_the_text_style_and_names_as_chips_without_quotes() {
        let text = Style::default().fg(Color::Yellow);
        let prose = Prose::new()
            .text("Delete ")
            .quoted("feat/login")
            .text(" in ")
            .name("~/src/dux")
            .text("?");
        let spans = prose_spans(&prose, text, &theme());
        let contents: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            contents,
            vec![
                "Delete ",
                "\u{a0}feat/login\u{a0}",
                " in ",
                "\u{a0}~/src/dux\u{a0}",
                "?"
            ]
        );
        assert_eq!(spans[0].style, text);
        assert_eq!(spans[1].style, theme().name_style());
        assert_eq!(spans[3].style, theme().name_style());
        let rendered: String = contents.concat();
        assert!(!rendered.contains('"'), "{rendered}");
    }

    #[test]
    fn an_info_row_is_its_label_then_the_name_as_a_chip() {
        let label = Style::default().fg(Color::Gray);
        let line = labelled_name(" Agent: ", "feat/login", label, &theme());
        let contents: Vec<&str> = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(contents, vec![" Agent: ", "\u{a0}feat/login\u{a0}"]);
        assert_eq!(line.spans[0].style, label);
        assert_eq!(line.spans[1].style, theme().name_style());
    }

    #[test]
    fn prose_lines_break_at_line_breaks_and_indent_every_line() {
        let prose = Prose::new()
            .text("Recreate at ")
            .name("~/wt")
            .text("?\n\nIf branch ")
            .quoted("main")
            .text(" exists.");
        let lines = prose_lines(&prose, " ", Style::default(), &theme());
        let texts: Vec<String> = lines
            .iter()
            .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(
            texts,
            vec![
                " Recreate at \u{a0}~/wt\u{a0}?",
                " ",
                " If branch \u{a0}main\u{a0} exists."
            ]
        );
        assert_eq!(lines[2].spans[2].style, theme().name_style());
    }
}
