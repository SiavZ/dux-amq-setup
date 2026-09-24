//! The per-session settings modal (fork d0e601c5, keyboard nav 32f46d2b,
//! mouse f1ec84d6, persist-before-commit 773a6b04).
//!
//! One surface for every per-agent knob: title, context mode, YOLO, the
//! custom system prompt, per-rule watch arm overrides, auto-clear on task
//! done, and the AMQ verify-envelope override. The modal edits a DRAFT; only
//! Save sends `Command::SetSessionSettings`, which persists before it touches
//! live memory, so a failed save leaves both memory and the draft intact for
//! a retry.
//!
//! House rules it follows (see `modal.rs`): a `Form` with a multiline field
//! and a Save button. The system-prompt field is engaged explicitly (Enter or
//! the engage key) and while engaged owns every key but the exit binding, so
//! Enter is a newline there, never Save.

use anyhow::Result;
use dux_core::engine::Command;
use dux_core::session_settings::{ContextMode, SessionSettings};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::components::button::{Button, ButtonKind, ButtonPressedTarget, button_state_for};
use super::input::contains_point;
use super::render::centered_rect_exact;
use super::text_input::TextInput;
use super::{App, InputTarget, OverlayMouseLayout, PromptState};
use crate::keybindings::{Action, BindingScope, text_field_owns_key};

/// Visible rows of the system-prompt editor.
const SYSTEM_PROMPT_LINES: usize = 4;

/// State for the open modal. Built from the engine's saved settings when it
/// opens; nothing is written until Save.
#[derive(Clone, Debug)]
pub(crate) struct SessionSettingsPrompt {
    pub(crate) session_id: String,
    /// Header label: the agent's display label at open time.
    pub(crate) session_label: String,
    pub(crate) draft: SessionSettings,
    pub(crate) draft_title: TextInput,
    /// Multiline editor for the custom system prompt. Mirrored into
    /// `draft.system_prompt` only on Save, so Cancel discards edits.
    pub(crate) draft_system_prompt: TextInput,
    pub(crate) focus: SettingsFocus,
    /// The provider's configured watch rules, with the effective arm state.
    /// Config-driven, so captured once at open.
    pub(crate) rules: Vec<WatchRuleSummary>,
    /// Hit-test rects published by the last render, in draw order. Kept on
    /// the prompt rather than in `OverlayMouseLayout` because that type is
    /// `Copy` and this list is variable-length.
    pub(crate) hit_rows: Vec<(Rect, SettingsFocus)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WatchRuleSummary {
    pub(crate) idx: usize,
    pub(crate) label: String,
    pub(crate) armed: bool,
}

/// Cursor position in the modal. Tab / Shift-Tab and Up / Down cycle it.
/// `WatchRule(i)` indexes into [`SessionSettingsPrompt::rules`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsFocus {
    Title,
    ModeAttended,
    ModeOrchestrator,
    ModeWorker,
    Yolo,
    SystemPrompt,
    WatchRule(usize),
    AutoClearOnDone,
    VerifyDefault,
    VerifyStrict,
    VerifySkip,
    SaveButton,
    CancelButton,
}

impl SettingsFocus {
    /// Next focus; skips the watch-rule rows when there are none, wraps.
    pub(crate) fn next(self, rules_len: usize) -> Self {
        match self {
            Self::Title => Self::ModeAttended,
            Self::ModeAttended => Self::ModeOrchestrator,
            Self::ModeOrchestrator => Self::ModeWorker,
            Self::ModeWorker => Self::Yolo,
            // Both spawn-time settings, so they sit together.
            Self::Yolo => Self::SystemPrompt,
            Self::SystemPrompt if rules_len > 0 => Self::WatchRule(0),
            Self::SystemPrompt => Self::AutoClearOnDone,
            Self::WatchRule(idx) if idx + 1 < rules_len => Self::WatchRule(idx + 1),
            Self::WatchRule(_) => Self::AutoClearOnDone,
            Self::AutoClearOnDone => Self::VerifyDefault,
            Self::VerifyDefault => Self::VerifyStrict,
            Self::VerifyStrict => Self::VerifySkip,
            Self::VerifySkip => Self::SaveButton,
            Self::SaveButton => Self::CancelButton,
            Self::CancelButton => Self::Title,
        }
    }

    /// Mirror of [`Self::next`].
    pub(crate) fn prev(self, rules_len: usize) -> Self {
        match self {
            Self::Title => Self::CancelButton,
            Self::ModeAttended => Self::Title,
            Self::ModeOrchestrator => Self::ModeAttended,
            Self::ModeWorker => Self::ModeOrchestrator,
            Self::Yolo => Self::ModeWorker,
            Self::SystemPrompt => Self::Yolo,
            Self::WatchRule(0) => Self::SystemPrompt,
            Self::WatchRule(idx) => Self::WatchRule(idx - 1),
            Self::AutoClearOnDone if rules_len > 0 => Self::WatchRule(rules_len - 1),
            Self::AutoClearOnDone => Self::SystemPrompt,
            Self::VerifyDefault => Self::AutoClearOnDone,
            Self::VerifyStrict => Self::VerifyDefault,
            Self::VerifySkip => Self::VerifyStrict,
            Self::SaveButton => Self::VerifySkip,
            Self::CancelButton => Self::SaveButton,
        }
    }
}

/// A watch rule's label: its own label, else the start of its pattern, else
/// its position.
pub(crate) fn watch_rule_display_label(idx: usize, label: &str, pattern: &str) -> String {
    let label = label.trim();
    if !label.is_empty() {
        return label.to_string();
    }
    let pattern = pattern.trim();
    if !pattern.is_empty() {
        return pattern.chars().take(64).collect();
    }
    format!("rule {idx}")
}

impl App {
    fn session_settings_prompt_mut(&mut self) -> Option<&mut SessionSettingsPrompt> {
        match &mut self.prompt {
            PromptState::SessionSettings(p) => Some(p),
            _ => None,
        }
    }

    /// Open the modal for the selected agent. With nothing selected it warns
    /// on the status line, so the binding is safe to press anywhere.
    pub(crate) fn open_session_settings(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_warning("Select an agent first, then open its session settings.");
            return Ok(());
        };
        let settings = self.engine.session_settings_or_default(&session.id);
        let rules = self.collect_provider_watch_rule_summaries(&session.provider, &settings);
        let title = session.title.clone().unwrap_or_default();
        self.input_target = InputTarget::None;
        self.fullscreen_overlay = super::FullscreenOverlay::None;
        self.prompt = PromptState::SessionSettings(Box::new(SessionSettingsPrompt {
            session_id: session.id.clone(),
            session_label: session.display_label(),
            draft_system_prompt: TextInput::with_text(
                settings.system_prompt.clone().unwrap_or_default(),
            )
            .with_multiline(SYSTEM_PROMPT_LINES),
            draft: settings,
            draft_title: TextInput::with_text(title)
                .with_char_map(dux_core::git::agent_name_char_map),
            // The first interactive control, not Title: arrow keys at Title
            // go to the text field, so a user reaching for Down first would
            // read the modal as frozen (32f46d2b).
            focus: SettingsFocus::ModeAttended,
            rules,
            hit_rows: Vec::new(),
        }));
        Ok(())
    }

    /// The provider's configured watch rules with the effective arm state:
    /// the saved per-session override, else armed (the config default).
    pub(crate) fn collect_provider_watch_rule_summaries(
        &self,
        provider: &dux_core::model::ProviderKind,
        settings: &SessionSettings,
    ) -> Vec<WatchRuleSummary> {
        self.engine
            .config
            .providers
            .get(provider.as_str())
            .map(|cfg| {
                cfg.watch
                    .iter()
                    .enumerate()
                    .map(|(idx, rule)| WatchRuleSummary {
                        idx,
                        label: watch_rule_display_label(idx, &rule.label, &rule.pattern),
                        armed: settings.watch_rule_arm.get(&idx).copied().unwrap_or(true),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Send the draft. On success the modal closes; on a store failure it
    /// stays open with the draft intact so the user can retry (773a6b04).
    pub(crate) fn save_session_settings(&mut self) -> Result<()> {
        let PromptState::SessionSettings(prompt) = &self.prompt else {
            return Ok(());
        };
        let mut settings = prompt.draft.clone();
        // Trailing whitespace dropped; all-whitespace means "none", so the
        // wrapper never receives an empty --append-system-prompt.
        let text = prompt.draft_system_prompt.text.trim_end();
        settings.system_prompt = (!text.trim().is_empty()).then(|| text.to_string());
        let title = prompt.draft_title.text.trim().to_string();
        let command = Command::SetSessionSettings {
            session_id: prompt.session_id.clone(),
            settings: Box::new(settings),
            title: Some((!title.is_empty()).then_some(title)),
        };
        match self.engine.apply(command) {
            Ok(reaction) => {
                self.prompt = PromptState::None;
                self.input_target = InputTarget::None;
                self.apply_reaction(reaction);
                self.rebuild_left_items();
            }
            Err(err) => {
                self.set_error(format!(
                    "Failed to save session settings: {err:#}. Nothing changed; \
                     fix the cause and press Save again, or Esc to discard."
                ));
            }
        }
        Ok(())
    }

    fn cancel_session_settings(&mut self) {
        self.prompt = PromptState::None;
        self.input_target = InputTarget::None;
    }

    fn set_session_settings_focus(&mut self, focus: SettingsFocus) {
        if focus != SettingsFocus::SystemPrompt {
            self.input_target = InputTarget::None;
        }
        if let Some(p) = self.session_settings_prompt_mut() {
            p.focus = focus;
        }
    }

    fn engage_session_settings_system_prompt(&mut self) {
        self.set_session_settings_focus(SettingsFocus::SystemPrompt);
        self.input_target = InputTarget::SessionSettingsPrompt;
        if let Some(p) = self.session_settings_prompt_mut() {
            p.draft_system_prompt.move_end();
        }
    }

    pub(super) fn session_settings_prompt_engaged(&self) -> bool {
        self.input_target == InputTarget::SessionSettingsPrompt
    }

    /// Act on a toggleable row: radios select, checkboxes flip.
    fn toggle_session_settings_row(&mut self, focus: SettingsFocus) {
        let Some(p) = self.session_settings_prompt_mut() else {
            return;
        };
        match focus {
            SettingsFocus::Title
            | SettingsFocus::SystemPrompt
            | SettingsFocus::SaveButton
            | SettingsFocus::CancelButton => {}
            SettingsFocus::ModeAttended => p.draft.mode = ContextMode::Attended,
            SettingsFocus::ModeOrchestrator => p.draft.mode = ContextMode::Orchestrator,
            SettingsFocus::ModeWorker => p.draft.mode = ContextMode::Worker,
            SettingsFocus::Yolo => p.draft.yolo_permissions = !p.draft.yolo_permissions,
            SettingsFocus::WatchRule(idx) => {
                if let Some(rule) = p.rules.iter_mut().find(|r| r.idx == idx) {
                    rule.armed = !rule.armed;
                    p.draft.watch_rule_arm.insert(idx, rule.armed);
                }
            }
            SettingsFocus::AutoClearOnDone => {
                p.draft.auto_clear_on_task_done = !p.draft.auto_clear_on_task_done;
            }
            SettingsFocus::VerifyDefault => p.draft.verify_envelope_override = None,
            SettingsFocus::VerifyStrict => p.draft.verify_envelope_override = Some(true),
            SettingsFocus::VerifySkip => p.draft.verify_envelope_override = Some(false),
        }
    }

    /// Enter / Space / click on a control: buttons fire, the system-prompt
    /// field engages, everything else toggles.
    fn activate_session_settings_row(&mut self, focus: SettingsFocus) -> Result<()> {
        match focus {
            SettingsFocus::SaveButton => self.save_session_settings()?,
            SettingsFocus::CancelButton => self.cancel_session_settings(),
            SettingsFocus::SystemPrompt => self.engage_session_settings_system_prompt(),
            SettingsFocus::Title => {}
            other => self.toggle_session_settings_row(other),
        }
        Ok(())
    }

    /// Key handling for the modal.
    pub(super) fn handle_session_settings_key(&mut self, key: KeyEvent) -> Result<bool> {
        // The engaged editor owns every key but the exit binding.
        if self.session_settings_prompt_engaged() {
            let lookup = self.bindings.lookup(&key, BindingScope::CommitInput);
            if lookup == Some(Action::ExitCommitInput) {
                self.input_target = InputTarget::None;
            } else if let Some(p) = self.session_settings_prompt_mut() {
                if lookup == Some(Action::ClearTextField) {
                    p.draft_system_prompt.clear();
                } else {
                    p.draft_system_prompt.handle_key(key);
                }
            }
            return Ok(false);
        }
        let Some((focus, rules_len)) = self
            .session_settings_prompt_mut()
            .map(|p| (p.focus, p.rules.len()))
        else {
            return Ok(false);
        };
        let title_focused = focus == SettingsFocus::Title;
        // A focused single-line field owns plain characters and the
        // horizontal arrows, so they never reach the bindings.
        let action = if title_focused && text_field_owns_key(key) {
            None
        } else {
            self.bindings
                .lookup(&key, BindingScope::Dialog)
                .or_else(|| self.bindings.lookup(&key, BindingScope::Palette))
        };
        match action {
            Some(Action::CloseOverlay) => self.cancel_session_settings(),
            Some(Action::ToggleSelection) => {
                let reverse = matches!(key.code, KeyCode::BackTab | KeyCode::Left)
                    || key
                        .modifiers
                        .contains(ratatui::crossterm::event::KeyModifiers::SHIFT)
                        && key.code == KeyCode::Tab;
                let next = if reverse {
                    focus.prev(rules_len)
                } else {
                    focus.next(rules_len)
                };
                self.set_session_settings_focus(next);
            }
            Some(Action::MoveDown) => self.set_session_settings_focus(focus.next(rules_len)),
            Some(Action::MoveUp) => self.set_session_settings_focus(focus.prev(rules_len)),
            // Enter submits from the single-line title (nothing competes for
            // it there) and acts on the focused control elsewhere.
            Some(Action::Confirm) if title_focused => self.save_session_settings()?,
            Some(Action::Confirm) => self.activate_session_settings_row(focus)?,
            _ if title_focused => {
                if let Some(p) = self.session_settings_prompt_mut() {
                    p.draft_title.handle_key(key);
                }
            }
            _ if key.code == KeyCode::Char(' ') => self.activate_session_settings_row(focus)?,
            // The vim-style engage key, resolved through the bindings.
            _ if focus == SettingsFocus::SystemPrompt
                && self.bindings.lookup(&key, BindingScope::Files)
                    == Some(Action::EngageCommitInput) =>
            {
                self.engage_session_settings_system_prompt();
            }
            _ => {}
        }
        Ok(false)
    }

    /// Paste lands in the focused text field only.
    pub(super) fn paste_into_session_settings(&mut self, text: &str) {
        let engaged = self.session_settings_prompt_engaged();
        if let Some(p) = self.session_settings_prompt_mut() {
            if engaged {
                p.draft_system_prompt.insert_str(text);
            } else if p.focus == SettingsFocus::Title {
                p.draft_title.insert_str(&text.replace(['\r', '\n'], " "));
            }
        }
    }

    /// A press inside the modal: which row it hit, if any. Title first
    /// (text-cursor placement), then the published rows.
    pub(super) fn session_settings_hit(&self, column: u16, row: u16) -> Option<SettingsFocus> {
        let PromptState::SessionSettings(p) = &self.prompt else {
            return None;
        };
        if let OverlayMouseLayout::SessionSettings { title_input, .. } = self.overlay_layout.active
            && contains_point(title_input, column, row)
        {
            return Some(SettingsFocus::Title);
        }
        p.hit_rows
            .iter()
            .find(|(rect, _)| contains_point(*rect, column, row))
            .map(|(_, focus)| *focus)
    }

    /// Mouse press on a row (f1ec84d6): focus it, then act on it. Title
    /// places the caret. Save and Cancel are buttons and go through the
    /// shared press/release path instead, so a press dragged off them does
    /// nothing.
    pub(super) fn click_session_settings_row(
        &mut self,
        focus: SettingsFocus,
        mouse: MouseEvent,
    ) -> Result<()> {
        self.set_session_settings_focus(focus);
        if focus == SettingsFocus::Title {
            if let OverlayMouseLayout::SessionSettings { title_input, .. } =
                self.overlay_layout.active
                && let Some(p) = self.session_settings_prompt_mut()
            {
                let col = usize::from(mouse.column.saturating_sub(title_input.x));
                p.draft_title.set_cursor_from_display_pos(0, col);
            }
            return Ok(());
        }
        self.activate_session_settings_row(focus)
    }

    /// A released Save or Cancel button.
    pub(super) fn press_session_settings_button(&mut self, save: bool) {
        if save {
            self.set_session_settings_focus(SettingsFocus::SaveButton);
            if let Err(err) = self.save_session_settings() {
                self.set_error(format!("{err:#}"));
            }
        } else {
            self.set_session_settings_focus(SettingsFocus::CancelButton);
            self.cancel_session_settings();
        }
    }

    // ── render ───────────────────────────────────────────────────────

    pub(super) fn render_session_settings(&mut self, frame: &mut Frame) {
        let PromptState::SessionSettings(prompt) = &self.prompt else {
            return;
        };
        let prompt = prompt.clone();
        let engaged = self.session_settings_prompt_engaged();
        let rule_rows = prompt.rules.len().max(1) as u16;
        // Header rows + fixed rows + editor + rules + buttons + hints.
        let height = 26 + rule_rows + SYSTEM_PROMPT_LINES as u16;
        let area = centered_rect_exact(86, height, frame.area());
        let title = format!("Session Settings: {}", prompt.session_label);
        let modal = self.open_modal_frame(frame, &title, area);
        let inner = modal.inner;
        let bottom = inner.y + inner.height;
        let mut y = inner.y;
        let mut rows: Vec<(Rect, SettingsFocus)> = Vec::new();

        let label = Style::default()
            .fg(self.theme.input_label_fg)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(self.theme.hint_dim_desc_fg);
        let text = Style::default().fg(self.theme.text_fg);
        let focused_style = self.theme.selection_style();

        let line = |frame: &mut Frame, y: &mut u16, content: Line<'static>| -> Rect {
            if *y >= bottom {
                return Rect::default();
            }
            let rect = Rect::new(inner.x + 1, *y, inner.width.saturating_sub(2), 1);
            Paragraph::new(content).render(rect, frame.buffer_mut());
            *y += 1;
            rect
        };
        let choice = |marker: &str, name: &str, desc: &str, focused: bool| -> Line<'static> {
            let style = if focused { focused_style } else { text };
            Line::from(vec![
                Span::styled(format!("  {marker} {name}"), style),
                Span::styled(format!("  {desc}"), dim),
            ])
        };
        let radio = |on: bool| if on { "(•)" } else { "( )" };
        let check = |on: bool| if on { "[x]" } else { "[ ]" };
        let f = prompt.focus;

        // Title.
        line(frame, &mut y, Line::from(Span::styled("Title", label)));
        let title_focused = f == SettingsFocus::Title;
        let title_rect = Rect::new(inner.x + 3, y, inner.width.saturating_sub(6), 1);
        let title_text = if prompt.draft_title.text.is_empty() {
            Span::styled("(branch name)", dim)
        } else {
            Span::styled(
                prompt.draft_title.text.clone(),
                if title_focused { focused_style } else { text },
            )
        };
        if y < bottom {
            Paragraph::new(Line::from(title_text)).render(title_rect, frame.buffer_mut());
            if title_focused {
                let (_, col) = prompt.draft_title.cursor_display_position();
                frame.set_cursor_position((title_rect.x + col as u16, y));
            }
            y += 1;
        }
        y += 1;

        // Context mode.
        line(
            frame,
            &mut y,
            Line::from(Span::styled(
                "Context mode (live; the Worker postscript applies from the next AMQ wake)",
                label,
            )),
        );
        for (focus, mode, name, desc) in [
            (
                SettingsFocus::ModeAttended,
                ContextMode::Attended,
                "attended",
                "you drive it; never auto-cleared",
            ),
            (
                SettingsFocus::ModeOrchestrator,
                ContextMode::Orchestrator,
                "orchestrator",
                "coordinates peers; gets checkpoint prompts",
            ),
            (
                SettingsFocus::ModeWorker,
                ContextMode::Worker,
                "worker",
                "task-done auto-clear, held while collaborating",
            ),
        ] {
            let r = line(
                frame,
                &mut y,
                choice(radio(prompt.draft.mode == mode), name, desc, f == focus),
            );
            rows.push((r, focus));
        }
        y += 1;

        // Spawn-time settings.
        line(
            frame,
            &mut y,
            Line::from(Span::styled(
                "Spawn-time (reconnect the agent to apply)",
                label,
            )),
        );
        let r = line(
            frame,
            &mut y,
            choice(
                check(prompt.draft.yolo_permissions),
                "YOLO",
                "skip permission prompts",
                f == SettingsFocus::Yolo,
            ),
        );
        rows.push((r, SettingsFocus::Yolo));
        let chars = prompt.draft_system_prompt.text.chars().count();
        let badge = if chars == 0 {
            "(none)".to_string()
        } else {
            format!("{chars} chars")
        };
        let r = line(
            frame,
            &mut y,
            choice(
                "",
                "System prompt",
                &badge,
                f == SettingsFocus::SystemPrompt,
            ),
        );
        rows.push((r, SettingsFocus::SystemPrompt));
        let editor_height = (SYSTEM_PROMPT_LINES as u16 + 2).min(bottom.saturating_sub(y));
        if editor_height > 2 {
            let field = Rect::new(inner.x + 3, y, inner.width.saturating_sub(6), editor_height);
            let exit_key = engaged.then(|| {
                self.bindings
                    .label_for_reaching(Action::ExitCommitInput, |_| true)
                    .unwrap_or_default()
            });
            let text_area = self.render_modal_text_field_frame(
                frame,
                field,
                f == SettingsFocus::SystemPrompt,
                exit_key,
            );
            let mut editor = prompt.draft_system_prompt.clone();
            editor.set_display_width((text_area.width > 0).then_some(text_area.width as usize));
            editor.set_visible_lines(text_area.height as usize);
            for (i, row_text) in editor.visible_lines().iter().enumerate() {
                if i >= text_area.height as usize {
                    break;
                }
                let r = Rect::new(text_area.x, text_area.y + i as u16, text_area.width, 1);
                Paragraph::new(row_text.as_str()).render(r, frame.buffer_mut());
            }
            if engaged {
                let (row, col) = editor.cursor_display_position();
                let (cx, cy) = (text_area.x + col as u16, text_area.y + row as u16);
                if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
                    frame.set_cursor_position((cx, cy));
                }
            }
            // A click anywhere on the editor focuses the system-prompt row.
            rows.push((field, SettingsFocus::SystemPrompt));
            y += editor_height;
        }
        y += 1;

        // Watch rules and auto-clear.
        line(
            frame,
            &mut y,
            Line::from(Span::styled(
                "Watch rules (live; applied on the next tick)",
                label,
            )),
        );
        if prompt.rules.is_empty() {
            line(
                frame,
                &mut y,
                Line::from(Span::styled(
                    "  (no rules configured for this provider)",
                    dim,
                )),
            );
        }
        for rule in &prompt.rules {
            let focus = SettingsFocus::WatchRule(rule.idx);
            let r = line(
                frame,
                &mut y,
                choice(check(rule.armed), &rule.label, "", f == focus),
            );
            rows.push((r, focus));
        }
        let r = line(
            frame,
            &mut y,
            choice(
                check(prompt.draft.auto_clear_on_task_done),
                "Auto-clear after task done",
                "Worker only; held while AMQ mail is pending",
                f == SettingsFocus::AutoClearOnDone,
            ),
        );
        rows.push((r, SettingsFocus::AutoClearOnDone));
        y += 1;

        // AMQ verify override.
        line(
            frame,
            &mut y,
            Line::from(Span::styled(
                "AMQ envelope verification (spawn-time)",
                label,
            )),
        );
        let v = prompt.draft.verify_envelope_override;
        for (focus, value, name, desc) in [
            (
                SettingsFocus::VerifyDefault,
                None,
                "default",
                "use [amq.inject] verify_envelope",
            ),
            (
                SettingsFocus::VerifyStrict,
                Some(true),
                "strict",
                "require a valid HMAC",
            ),
            (
                SettingsFocus::VerifySkip,
                Some(false),
                "skip",
                "accept unsigned bodies",
            ),
        ] {
            let r = line(
                frame,
                &mut y,
                choice(radio(v == value), name, desc, f == focus),
            );
            rows.push((r, focus));
        }

        // Cancel / Save, behind a blank row (misclick safety).
        let buttons_y = bottom.saturating_sub(4);
        let btn = 16u16;
        let gap = 6u16;
        let left = inner.x + inner.width.saturating_sub(btn * 2 + gap) / 2;
        let cancel = Rect::new(left, buttons_y, btn, 3);
        let save = Rect::new(left + btn + gap, buttons_y, btn, 3);
        if buttons_y > y {
            Button::new("Cancel")
                .kind(ButtonKind::Confirm)
                .state(button_state_for(
                    ButtonPressedTarget::SessionSettingsCancel,
                    self.pressed_button,
                    f == SettingsFocus::CancelButton,
                    true,
                ))
                .render(frame, cancel, &self.theme);
            Button::new("Save")
                .kind(ButtonKind::Confirm)
                .state(button_state_for(
                    ButtonPressedTarget::SessionSettingsSave,
                    self.pressed_button,
                    f == SettingsFocus::SaveButton,
                    true,
                ))
                .render(frame, save, &self.theme);
        }

        // Footer: every key through the bindings.
        let hint = if engaged {
            format!(
                "{} stop editing",
                self.bindings.labels_for(Action::ExitCommitInput)
            )
        } else {
            format!(
                "{} move  Space/{} act  {} close",
                self.bindings.labels_for(Action::ToggleSelection),
                self.bindings.label_for(Action::Confirm),
                self.bindings.label_for(Action::CloseOverlay),
            )
        };
        let hint_rect = Rect::new(
            inner.x + 1,
            bottom.saturating_sub(1),
            inner.width.saturating_sub(2),
            1,
        );
        Paragraph::new(Span::styled(hint, dim)).render(hint_rect, frame.buffer_mut());

        self.overlay_layout.active = OverlayMouseLayout::SessionSettings {
            title_input: title_rect,
            save_button: save,
            cancel_button: cancel,
        };
        if let PromptState::SessionSettings(p) = &mut self.prompt {
            p.hit_rows = rows;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};

    /// A test App whose single agent ("session-1", codex) is selected and
    /// has a DB row, so saves can land.
    fn app() -> App {
        let app = test_app(default_bindings());
        app.engine
            .session_store
            .create_session(&app.engine.sessions[0])
            .expect("seed session row");
        app
    }

    fn prompt(app: &App) -> &SessionSettingsPrompt {
        match &app.prompt {
            PromptState::SessionSettings(p) => p,
            other => panic!("expected SessionSettings, got {other:?}"),
        }
    }

    fn prompt_mut(app: &mut App) -> &mut SessionSettingsPrompt {
        match &mut app.prompt {
            PromptState::SessionSettings(p) => p,
            other => panic!("expected SessionSettings, got {other:?}"),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn click(app: &mut App, column: u16, row: u16) {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
        }
    }

    fn render(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("draw");
    }

    fn row_rect(app: &App, focus: SettingsFocus) -> Rect {
        prompt(app)
            .hit_rows
            .iter()
            .find(|(_, f)| *f == focus)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("{focus:?} not published"))
    }

    fn open_rendered() -> App {
        let mut app = app();
        app.open_session_settings().expect("open");
        render(&mut app);
        app
    }

    #[test]
    fn settings_focus_navigation_skips_empty_watch_rules() {
        use SettingsFocus as F;
        assert_eq!(F::Yolo.next(0), F::SystemPrompt);
        assert_eq!(F::Yolo.next(3), F::SystemPrompt);
        assert_eq!(F::SystemPrompt.next(0), F::AutoClearOnDone);
        assert_eq!(F::SystemPrompt.next(3), F::WatchRule(0));
        assert_eq!(F::WatchRule(2).next(3), F::AutoClearOnDone);
        assert_eq!(F::CancelButton.next(0), F::Title);
        assert_eq!(F::AutoClearOnDone.prev(0), F::SystemPrompt);
        assert_eq!(F::AutoClearOnDone.prev(3), F::WatchRule(2));
        assert_eq!(F::WatchRule(0).prev(3), F::SystemPrompt);
        assert_eq!(F::SystemPrompt.prev(0), F::Yolo);
        // Every stop is reachable forwards and back.
        let mut f = F::Title;
        for _ in 0..20 {
            assert_eq!(f.next(2).prev(2), f);
            f = f.next(2);
        }
    }

    #[test]
    fn open_session_settings_warns_when_no_session_selected() {
        let mut app = app();
        app.selected_left = 99;
        app.open_session_settings().expect("open");
        assert!(matches!(app.prompt, PromptState::None));
        assert!(!app.status.message().is_empty(), "a warning is shown");
    }

    #[test]
    fn open_session_settings_seeds_draft_from_live_session() {
        let mut app = app();
        let saved = SessionSettings {
            mode: ContextMode::Worker,
            yolo_permissions: true,
            auto_clear_on_task_done: true,
            system_prompt: Some("speak like a pirate".into()),
            ..SessionSettings::default()
        };
        app.engine
            .apply(Command::SetSessionSettings {
                session_id: "session-1".into(),
                settings: Box::new(saved.clone()),
                title: None,
            })
            .unwrap();
        app.open_session_settings().expect("open");
        let p = prompt(&app);
        assert_eq!(p.session_id, "session-1");
        assert_eq!(p.draft, saved);
        assert_eq!(p.draft_system_prompt.text, "speak like a pirate");
        // The first interactive control, not Title (32f46d2b).
        assert_eq!(p.focus, SettingsFocus::ModeAttended);
    }

    #[test]
    fn provider_watch_rule_summary_uses_pattern_when_label_missing() {
        let mut app = app();
        let mut cfg = app.engine.config.providers.get("codex").cloned().unwrap();
        cfg.watch = vec![dux_core::watch::WatchRule {
            pattern: "API Error.*rate limit".into(),
            label: String::new(),
            ..Default::default()
        }];
        app.engine
            .config
            .providers
            .commands
            .insert("codex".into(), cfg);
        let summaries = app.collect_provider_watch_rule_summaries(
            &dux_core::model::ProviderKind::from_str("codex"),
            &SessionSettings::default(),
        );
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].label, "API Error.*rate limit");
        assert!(summaries[0].armed);
        assert_eq!(watch_rule_display_label(3, " ", " "), "rule 3");
    }

    #[test]
    fn save_session_settings_persists_to_sqlite_and_in_memory() {
        let mut app = app();
        app.open_session_settings().unwrap();
        {
            let p = prompt_mut(&mut app);
            p.draft.mode = ContextMode::Worker;
            p.draft.auto_clear_on_task_done = true;
            p.draft.yolo_permissions = true;
            p.draft.verify_envelope_override = Some(true);
            p.draft_system_prompt
                .set_text("be concise and cite sources".into());
            p.draft_title.set_text("Reviewer".into());
        }
        app.save_session_settings().unwrap();
        assert!(matches!(app.prompt, PromptState::None), "modal closed");
        let expected = SessionSettings {
            mode: ContextMode::Worker,
            auto_clear_on_task_done: true,
            yolo_permissions: true,
            verify_envelope_override: Some(true),
            system_prompt: Some("be concise and cite sources".into()),
            ..SessionSettings::default()
        };
        assert_eq!(app.engine.session_settings("session-1"), Some(&expected));
        let stored = app.engine.session_store.load_session_settings().unwrap();
        assert_eq!(stored["session-1"], expected);
        assert_eq!(app.engine.sessions[0].title.as_deref(), Some("Reviewer"));
    }

    #[test]
    fn save_session_settings_system_prompt_whitespace_persists_as_none() {
        let mut app = app();
        app.open_session_settings().unwrap();
        prompt_mut(&mut app)
            .draft_system_prompt
            .set_text("   \n\t  \n".into());
        prompt_mut(&mut app).draft.yolo_permissions = true;
        app.save_session_settings().unwrap();
        let saved = app.engine.session_settings("session-1").unwrap();
        assert_eq!(saved.system_prompt, None);
    }

    #[test]
    fn save_session_settings_system_prompt_change_triggers_respawn_warning() {
        let mut app = app();
        app.open_session_settings().unwrap();
        prompt_mut(&mut app)
            .draft_system_prompt
            .set_text("review pls".into());
        app.save_session_settings().unwrap();
        let msg = app.status.message().to_string();
        assert!(msg.contains("system prompt"), "{msg}");
        assert!(msg.contains("Reconnect"), "{msg}");
    }

    #[test]
    fn save_session_settings_no_changes_reports_no_changes() {
        let mut app = app();
        app.open_session_settings().unwrap();
        app.save_session_settings().unwrap();
        let msg = app.status.message().to_string();
        assert!(msg.contains("no changes"), "{msg}");
    }

    /// 773a6b04 (audit03 P1-16): a failed store write leaves live memory as
    /// it was, and the modal stays open with the draft for a retry.
    #[test]
    fn failed_settings_upsert_preserves_memory_runtime_and_retryable_draft() {
        let mut app = app();
        app.open_session_settings().unwrap();
        prompt_mut(&mut app).draft.mode = ContextMode::Worker;
        prompt_mut(&mut app).draft.auto_clear_on_task_done = true;
        // Every write now fails: the row the save targets is gone.
        app.engine
            .session_store
            .delete_session("session-1")
            .expect("drop the row");

        app.save_session_settings().unwrap();

        assert_eq!(app.engine.session_settings("session-1"), None);
        assert!(app.engine.watch_rule_rows().is_empty());
        let p = prompt(&app);
        assert_eq!(p.draft.mode, ContextMode::Worker, "draft kept for retry");
        assert!(p.draft.auto_clear_on_task_done);
        assert!(
            app.status.message().contains("Failed to save"),
            "{}",
            app.status.message()
        );
    }

    #[test]
    fn keyboard_space_toggles_and_tab_moves_focus() {
        let mut app = app();
        app.open_session_settings().unwrap();
        // Down to Worker, Space selects it.
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Down)).unwrap();
        app.handle_key(key(KeyCode::Char(' '))).unwrap();
        assert_eq!(prompt(&app).draft.mode, ContextMode::Worker);
        // Up from ModeAttended reaches Title, where typing edits the title.
        app.handle_key(key(KeyCode::Up)).unwrap();
        app.handle_key(key(KeyCode::Up)).unwrap();
        app.handle_key(key(KeyCode::Up)).unwrap();
        assert_eq!(prompt(&app).focus, SettingsFocus::Title);
        app.handle_key(key(KeyCode::Char('x'))).unwrap();
        assert!(prompt(&app).draft_title.text.ends_with('x'));
        // Esc discards.
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(app.engine.session_settings("session-1"), None);
    }

    /// Enter in the engaged system-prompt editor is a newline, never Save;
    /// the exit key leaves the editor and the next Enter re-engages it.
    #[test]
    fn engaged_system_prompt_owns_enter() {
        let mut app = app();
        app.open_session_settings().unwrap();
        prompt_mut(&mut app).focus = SettingsFocus::SystemPrompt;
        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert!(app.session_settings_prompt_engaged());
        for c in ['a', 'b'] {
            app.handle_key(key(KeyCode::Char(c))).unwrap();
            app.handle_key(key(KeyCode::Enter)).unwrap();
        }
        assert_eq!(prompt(&app).draft_system_prompt.text, "a\nb\n");
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert!(!app.session_settings_prompt_engaged());
        assert!(matches!(app.prompt, PromptState::SessionSettings(_)));
    }

    #[test]
    fn ctrl_shift_s_opens_the_modal_from_anywhere() {
        let mut app = app();
        app.handle_key(KeyEvent::new(
            KeyCode::Char('S'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ))
        .unwrap();
        assert!(matches!(app.prompt, PromptState::SessionSettings(_)));
    }

    #[test]
    fn palette_command_opens_the_modal() {
        let mut app = app();
        app.execute_command("session-settings".into()).unwrap();
        assert!(matches!(app.prompt, PromptState::SessionSettings(_)));
    }

    #[test]
    fn render_session_settings_publishes_overlay_layout_with_rows() {
        let app = open_rendered();
        let OverlayMouseLayout::SessionSettings {
            title_input,
            save_button,
            cancel_button,
        } = app.overlay_layout.active
        else {
            panic!("expected the SessionSettings layout");
        };
        assert!(title_input.width > 0 && save_button.width > 0 && cancel_button.width > 0);
        let focuses: Vec<SettingsFocus> = prompt(&app).hit_rows.iter().map(|(_, f)| *f).collect();
        for required in [
            SettingsFocus::ModeAttended,
            SettingsFocus::ModeOrchestrator,
            SettingsFocus::ModeWorker,
            SettingsFocus::Yolo,
            SettingsFocus::SystemPrompt,
            SettingsFocus::AutoClearOnDone,
            SettingsFocus::VerifyDefault,
            SettingsFocus::VerifyStrict,
            SettingsFocus::VerifySkip,
        ] {
            assert!(
                focuses.contains(&required),
                "{required:?} missing: {focuses:?}"
            );
        }
    }

    #[test]
    fn mouse_click_session_settings_radio_selects_mode() {
        let mut app = open_rendered();
        let r = row_rect(&app, SettingsFocus::ModeWorker);
        click(&mut app, r.x + 3, r.y);
        assert_eq!(prompt(&app).focus, SettingsFocus::ModeWorker);
        assert_eq!(prompt(&app).draft.mode, ContextMode::Worker);
    }

    #[test]
    fn mouse_click_session_settings_checkbox_focuses_and_toggles() {
        let mut app = open_rendered();
        let r = row_rect(&app, SettingsFocus::Yolo);
        click(&mut app, r.x + 3, r.y);
        assert_eq!(prompt(&app).focus, SettingsFocus::Yolo);
        assert!(prompt(&app).draft.yolo_permissions);
    }

    #[test]
    fn mouse_click_session_settings_title_focuses_and_positions_cursor() {
        let mut app = app();
        app.open_session_settings().unwrap();
        prompt_mut(&mut app)
            .draft_title
            .set_text("draft title".into());
        render(&mut app);
        let OverlayMouseLayout::SessionSettings { title_input, .. } = app.overlay_layout.active
        else {
            panic!("layout");
        };
        click(&mut app, title_input.x + 3, title_input.y);
        let p = prompt(&app);
        assert_eq!(p.focus, SettingsFocus::Title);
        assert_eq!(p.draft_title.cursor, 3);
        assert_eq!(p.draft, SessionSettings::default(), "nothing toggled");
    }

    #[test]
    fn mouse_click_session_settings_outside_rect_is_noop() {
        let mut app = open_rendered();
        let before = (prompt(&app).focus, prompt(&app).draft.clone());
        // The blank row between the title and the context-mode heading.
        let OverlayMouseLayout::SessionSettings { title_input, .. } = app.overlay_layout.active
        else {
            panic!("layout");
        };
        click(&mut app, title_input.x + 2, title_input.y + 1);
        let p = prompt(&app);
        assert_eq!((p.focus, p.draft.clone()), before);
    }

    #[test]
    fn mouse_click_session_settings_save_button_fires_save() {
        let mut app = open_rendered();
        prompt_mut(&mut app).draft.mode = ContextMode::Orchestrator;
        let OverlayMouseLayout::SessionSettings { save_button, .. } = app.overlay_layout.active
        else {
            panic!("layout");
        };
        click(&mut app, save_button.x + 2, save_button.y + 1);
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(
            app.engine.session_mode("session-1"),
            ContextMode::Orchestrator
        );
    }

    #[test]
    fn mouse_click_session_settings_cancel_button_dismisses_modal() {
        let mut app = open_rendered();
        prompt_mut(&mut app).draft.mode = ContextMode::Orchestrator;
        let OverlayMouseLayout::SessionSettings { cancel_button, .. } = app.overlay_layout.active
        else {
            panic!("layout");
        };
        click(&mut app, cancel_button.x + 2, cancel_button.y + 1);
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(app.engine.session_mode("session-1"), ContextMode::Attended);
    }
}
