use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CheckboxState {
    Normal,
    Hovered,
    Focused,
    Disabled,
}

#[derive(Clone, Debug)]
pub(crate) struct CheckboxLayout {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) height: u16,
    background: Option<Color>,
}

impl CheckboxLayout {
    pub(crate) fn empty() -> Self {
        Self {
            lines: Vec::new(),
            height: 0,
            background: None,
        }
    }

    pub(crate) fn background(mut self, background: Color) -> Self {
        self.background = Some(background);
        self
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Checkbox<'a> {
    label: &'a str,
    checked: bool,
    state: CheckboxState,
}

impl<'a> Checkbox<'a> {
    const PREFIX: &'static str = " ";
    const GAP: &'static str = " ";
    const INDENT: &'static str = "     ";

    pub(crate) const fn indent() -> &'static str {
        Self::INDENT
    }

    pub(crate) fn new(label: &'a str) -> Self {
        Self {
            label,
            checked: false,
            state: CheckboxState::Normal,
        }
    }

    pub(crate) fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub(crate) fn state(mut self, state: CheckboxState) -> Self {
        self.state = state;
        self
    }

    pub(crate) fn layout(
        &self,
        max_width: u16,
        marker_style: Style,
        label_style: Style,
    ) -> CheckboxLayout {
        if max_width == 0 {
            return CheckboxLayout::empty();
        }

        let indent_width = display_width(Self::INDENT);
        let available = usize::from(max_width);
        let label_width = available.saturating_sub(indent_width).max(1);
        let wrapped = wrap_checkbox_label(self.label, label_width);
        let marker = if self.checked { "[x]" } else { "[ ]" };
        let mut lines = Vec::with_capacity(wrapped.len().max(1));

        for (index, text) in wrapped.into_iter().enumerate() {
            if index == 0 {
                lines.push(Line::from(vec![
                    Span::raw(Self::PREFIX),
                    Span::styled(marker.to_string(), marker_style),
                    Span::raw(Self::GAP),
                    Span::styled(text, label_style),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw(Self::INDENT),
                    Span::styled(text, label_style),
                ]));
            }
        }

        CheckboxLayout {
            height: lines.len() as u16,
            lines,
            background: None,
        }
    }

    pub(crate) fn inline_prefix(&self, marker_style: Style) -> Vec<Span<'static>> {
        vec![
            Span::raw(Self::PREFIX),
            Span::styled(
                if self.checked { "[x]" } else { "[ ]" }.to_string(),
                self.marker_style(marker_style),
            ),
            Span::raw(Self::GAP),
        ]
    }

    pub(crate) fn marker_style(&self, base_marker_style: Style) -> Style {
        match self.state {
            CheckboxState::Normal => base_marker_style,
            CheckboxState::Hovered => base_marker_style.add_modifier(Modifier::BOLD),
            CheckboxState::Focused => base_marker_style.add_modifier(Modifier::BOLD),
            CheckboxState::Disabled => base_marker_style,
        }
    }

    pub(crate) fn label_style(&self, base_label_style: Style) -> Style {
        match self.state {
            CheckboxState::Normal => base_label_style,
            CheckboxState::Hovered => base_label_style.add_modifier(Modifier::BOLD),
            CheckboxState::Focused => base_label_style.add_modifier(Modifier::BOLD),
            CheckboxState::Disabled => base_label_style,
        }
    }
}

impl Widget for CheckboxLayout {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let clear_style = self
            .background
            .map(|background| Style::default().bg(background))
            .unwrap_or_default();
        for clear_offset in 0..area.height {
            let y = area.y.saturating_add(clear_offset);
            for x_offset in 0..area.width {
                let cell = &mut buf[(area.x.saturating_add(x_offset), y)];
                cell.reset();
                cell.set_style(clear_style);
            }
        }

        for (offset, line) in self.lines.into_iter().enumerate() {
            let y = area.y.saturating_add(offset as u16);
            if y >= area.y.saturating_add(area.height) {
                break;
            }
            line.render(
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
                buf,
            );
        }
    }
}

/// Terminal-cell width of `s`, reusing the renderer's own calculation so
/// double-width CJK, emoji, and combining marks are measured by the columns
/// they occupy rather than by Unicode scalar count.
fn display_width(s: &str) -> usize {
    Line::from(s).width()
}

fn wrap_checkbox_label(label: &str, max_width: usize) -> Vec<String> {
    if label.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for paragraph in label.split('\n') {
        let mut current = String::new();
        let mut current_width = 0usize;
        for word in paragraph.split_whitespace() {
            let word_width = display_width(word);
            if current.is_empty() {
                if word_width <= max_width {
                    current.push_str(word);
                    current_width = word_width;
                } else {
                    push_broken_word_lines(word, max_width, &mut lines);
                }
                continue;
            }

            let next_width = current_width + 1 + word_width;
            if next_width <= max_width {
                current.push(' ');
                current.push_str(word);
                current_width = next_width;
            } else {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
                if word_width <= max_width {
                    current.push_str(word);
                    current_width = word_width;
                } else {
                    push_broken_word_lines(word, max_width, &mut lines);
                }
            }
        }

        if !current.is_empty() {
            lines.push(current);
        } else if paragraph.is_empty() {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_broken_word_lines(word: &str, max_width: usize, lines: &mut Vec<String>) {
    let mut chunk = String::new();
    let mut chunk_width = 0usize;
    for ch in word.chars() {
        let ch_width = display_width(ch.encode_utf8(&mut [0u8; 4]));
        // Flush before a char that would overflow the line. A double-width
        // glyph can push the running width past `max_width` by one, so use a
        // "would exceed" check rather than exact equality.
        if chunk_width + ch_width > max_width && !chunk.is_empty() {
            lines.push(std::mem::take(&mut chunk));
            chunk_width = 0;
        }
        chunk.push(ch);
        chunk_width += ch_width;
    }
    if !chunk.is_empty() {
        lines.push(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkbox_wraps_label_with_continuation_indent() {
        let checkbox = Checkbox::new("Also delete the worktree and branch")
            .checked(false)
            .state(CheckboxState::Normal);

        let layout = checkbox.layout(20, Style::default(), Style::default());

        assert_eq!(layout.height, 3);
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.lines[0].spans[0].content.as_ref(), " ");
        assert_eq!(layout.lines[0].spans[1].content.as_ref(), "[ ]");
        assert_eq!(layout.lines[1].spans[0].content.as_ref(), "     ");
        assert_eq!(layout.lines[2].spans[0].content.as_ref(), "     ");
    }

    #[test]
    fn wrap_breaks_double_width_words_by_terminal_columns() {
        // Six double-width CJK chars = 12 columns. At max_width 5 the broken
        // chunks must not exceed 5 columns each: 2 chars (4 cols) per line,
        // since a third would reach 6 > 5.
        let lines = wrap_checkbox_label("一二三四五六", 5);
        for line in &lines {
            assert!(
                display_width(line) <= 5,
                "chunk {line:?} exceeds 5 columns ({} wide)",
                display_width(line)
            );
        }
        assert_eq!(lines.concat(), "一二三四五六");
    }

    #[test]
    fn wrap_wraps_cjk_words_at_column_boundary() {
        // "世界" and "日本" are 4 columns each. At max_width 5 they cannot
        // share a line (4 + 1 space + 4 = 9 > 5), so each gets its own line.
        // A scalar-count implementation would see 2 + 1 + 2 = 5 <= 5 and pack
        // both onto one line, so this asserts the display-width behavior.
        let lines = wrap_checkbox_label("世界 日本", 5);
        assert_eq!(lines, vec!["世界".to_string(), "日本".to_string()]);
    }

    #[test]
    fn checkbox_hover_and_focus_bolden_label_and_marker() {
        let marker = Style::default();
        let label = Style::default();
        let hovered = Checkbox::new("Label").state(CheckboxState::Hovered);
        let focused = Checkbox::new("Label").state(CheckboxState::Focused);

        assert!(
            hovered
                .marker_style(marker)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            focused
                .label_style(label)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    #[test]
    fn checkbox_inline_prefix_uses_shared_ascii_marker() {
        let unchecked = Checkbox::new("")
            .checked(false)
            .state(CheckboxState::Normal);
        let checked = Checkbox::new("").checked(true).state(CheckboxState::Normal);

        let unchecked_spans = unchecked.inline_prefix(Style::default());
        let checked_spans = checked.inline_prefix(Style::default());

        assert_eq!(unchecked_spans[1].content.as_ref(), "[ ]");
        assert_eq!(checked_spans[1].content.as_ref(), "[x]");
    }

    #[test]
    fn checkbox_indent_width_matches_indent_text() {
        assert_eq!(Checkbox::indent().chars().count(), 5);
    }

    #[test]
    fn checkbox_render_preserves_configured_background() {
        let layout = Checkbox::new("Label")
            .layout(
                12,
                Style::default().fg(Color::Yellow),
                Style::default().fg(Color::White),
            )
            .background(Color::Rgb(12, 34, 56));
        let mut buffer = Buffer::empty(Rect::new(0, 0, 12, 1));

        layout.render(buffer.area, &mut buffer);

        for x in 0..buffer.area.width {
            assert_eq!(buffer[(x, 0)].bg, Color::Rgb(12, 34, 56));
        }
    }
}
