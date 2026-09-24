//! The new-agent modal's harness picker and Advanced section (port of fork
//! 6448c3f5 "configure agents at creation" and 3a8ad183 "wire new agent
//! advanced mouse controls").
//!
//! The fork let an agent be configured as it is created: which harness it
//! runs, and its session settings (context mode, YOLO, watch-rule arms,
//! auto-clear, AMQ verification). The settings half reuses maple's session
//! settings: the rows are [`SettingsFocus`] values, a toggle is
//! [`apply_settings_toggle`], and the draft is applied after the create
//! commits through `Command::SetSessionSettings`, the same persist-before-
//! mutate path the settings modal's Save uses.
//!
//! The custom system prompt is deliberately not offered here. It is a
//! multiline field, and a multiline field would oblige this single-line Form
//! modal to grow Save/Cancel buttons (the dual-mode rule in `modal.rs`); it
//! stays one keystroke away in Session Settings once the agent exists.

use dux_core::model::ProviderKind;
use dux_core::session_settings::SessionSettings;
use dux_core::worker::CreateAgentRequest;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use super::session_settings::{SettingsFocus, WatchRuleSummary, apply_settings_toggle};
use super::{App, NameNewAgentFocus, PromptState};

/// Everything the new-agent modal holds beyond the name and its two
/// checkboxes. Boxed on the prompt so the enum stays small.
#[derive(Clone, Debug, Default)]
pub(crate) struct NewAgentExtras {
    /// Configured harnesses, in config (picker) order.
    pub(crate) provider_options: Vec<ProviderKind>,
    pub(crate) selected_provider: usize,
    /// The harness the request would launch unless the user picks another.
    pub(crate) initial_provider: Option<ProviderKind>,
    pub(crate) show_advanced: bool,
    pub(crate) draft: SessionSettings,
    /// The selected harness's watch rules with their draft arm state.
    pub(crate) rules: Vec<WatchRuleSummary>,
    /// Hit rects of the Provider/Advanced/settings rows, published by the
    /// last render (the mouse layout type is `Copy`, so a variable-length
    /// list lives here, as the settings modal does it).
    pub(crate) hit_rows: Vec<(Rect, NameNewAgentFocus)>,
}

impl NewAgentExtras {
    pub(crate) fn selected(&self) -> Option<&ProviderKind> {
        self.provider_options.get(self.selected_provider)
    }

    /// The setting rows the Advanced section shows, in focus order.
    pub(crate) fn setting_rows(&self) -> Vec<SettingsFocus> {
        let mut rows = vec![
            SettingsFocus::ModeAttended,
            SettingsFocus::ModeOrchestrator,
            SettingsFocus::ModeWorker,
            SettingsFocus::Yolo,
        ];
        rows.extend(self.rules.iter().map(|r| SettingsFocus::WatchRule(r.idx)));
        rows.extend([
            SettingsFocus::AutoClearOnDone,
            SettingsFocus::VerifyDefault,
            SettingsFocus::VerifyStrict,
            SettingsFocus::VerifySkip,
        ]);
        rows
    }
}

/// The harness a create request launches unless overridden.
pub(crate) fn request_provider(request: &CreateAgentRequest) -> ProviderKind {
    match request {
        CreateAgentRequest::NewProject { project, .. }
        | CreateAgentRequest::PullRequest { project, .. }
        | CreateAgentRequest::ExistingManagedWorktree { project, .. }
        | CreateAgentRequest::ForkExternalWorktree { project, .. }
        | CreateAgentRequest::SharedWorkspace { project, .. } => project.default_provider.clone(),
        CreateAgentRequest::ForkSession { source_session, .. } => source_session.provider.clone(),
        CreateAgentRequest::Standalone { provider, .. } => provider.clone(),
    }
}

/// Point a create request at `provider`. Every managed create resolves its
/// harness from the request's own project copy (or the fork source), so
/// overriding that copy changes the launch and nothing else: the registered
/// project and its configured default are untouched.
pub(crate) fn set_request_provider(request: &mut CreateAgentRequest, provider: ProviderKind) {
    match request {
        CreateAgentRequest::NewProject { project, .. }
        | CreateAgentRequest::PullRequest { project, .. }
        | CreateAgentRequest::ExistingManagedWorktree { project, .. }
        | CreateAgentRequest::ForkExternalWorktree { project, .. }
        | CreateAgentRequest::SharedWorkspace { project, .. } => {
            project.default_provider = provider;
        }
        CreateAgentRequest::ForkSession { source_session, .. } => {
            source_session.provider = provider;
        }
        CreateAgentRequest::Standalone {
            provider: current, ..
        } => *current = provider,
    }
}

/// Identifies which create a creation-time settings draft belongs to, so a
/// draft left behind by an abandoned confirm step can never attach itself to
/// some later, unrelated agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreateTag {
    project_id: Option<String>,
    name: Option<String>,
}

impl CreateTag {
    pub(crate) fn of(request: &CreateAgentRequest) -> Self {
        let name = match request {
            CreateAgentRequest::NewProject { custom_name, .. }
            | CreateAgentRequest::PullRequest { custom_name, .. }
            | CreateAgentRequest::ForkSession { custom_name, .. }
            | CreateAgentRequest::ExistingManagedWorktree { custom_name, .. }
            | CreateAgentRequest::ForkExternalWorktree { custom_name, .. }
            | CreateAgentRequest::SharedWorkspace { custom_name, .. } => custom_name.clone(),
            CreateAgentRequest::Standalone { title, .. } => Some(title.clone()),
        };
        Self {
            project_id: request.project_id().map(str::to_string),
            name,
        }
    }
}

/// One line of the harness/Advanced block: a heading, or a focusable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExtrasLine {
    Heading(&'static str),
    Row {
        target: NameNewAgentFocus,
        marker: String,
        name: String,
        desc: String,
    },
}

/// The block's lines, in draw (and focus) order. Headings mirror the
/// settings modal's sections so the two read the same.
pub(crate) fn extras_lines(extras: &NewAgentExtras) -> Vec<ExtrasLine> {
    use dux_core::session_settings::ContextMode;
    let radio = |on: bool| if on { "(\u{2022})" } else { "( )" }.to_string();
    let check = |on: bool| if on { "[x]" } else { "[ ]" }.to_string();
    let row = |target, marker, name: &str, desc: &str| ExtrasLine::Row {
        target,
        marker,
        name: name.to_string(),
        desc: desc.to_string(),
    };
    let harness = extras
        .selected()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "(none)".to_string());
    let mut lines = vec![
        row(
            NameNewAgentFocus::Provider,
            "Harness:".to_string(),
            &harness,
            "Space for the next one",
        ),
        row(
            NameNewAgentFocus::AdvancedToggle,
            check(extras.show_advanced),
            "Advanced settings",
            "context mode, YOLO, watch rules, AMQ",
        ),
    ];
    if !extras.show_advanced {
        return lines;
    }
    let d = &extras.draft;
    lines.push(ExtrasLine::Heading("Context mode"));
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
            "coordinates peers",
        ),
        (
            SettingsFocus::ModeWorker,
            ContextMode::Worker,
            "worker",
            "task-done auto-clear",
        ),
    ] {
        lines.push(row(
            NameNewAgentFocus::Setting(focus),
            radio(d.mode == mode),
            name,
            desc,
        ));
    }
    lines.push(row(
        NameNewAgentFocus::Setting(SettingsFocus::Yolo),
        check(d.yolo_permissions),
        "YOLO",
        "skip permission prompts (from the next launch)",
    ));
    lines.push(ExtrasLine::Heading("Watch rules"));
    for rule in &extras.rules {
        lines.push(row(
            NameNewAgentFocus::Setting(SettingsFocus::WatchRule(rule.idx)),
            check(rule.armed),
            &rule.label,
            "",
        ));
    }
    lines.push(row(
        NameNewAgentFocus::Setting(SettingsFocus::AutoClearOnDone),
        check(d.auto_clear_on_task_done),
        "Auto-clear after task done",
        "Worker only",
    ));
    lines.push(ExtrasLine::Heading("AMQ envelope verification"));
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
        lines.push(row(
            NameNewAgentFocus::Setting(focus),
            radio(d.verify_envelope_override == value),
            name,
            desc,
        ));
    }
    lines
}

impl App {
    /// The extras for a freshly opened new-agent modal.
    pub(crate) fn new_agent_extras_for(&self, request: &CreateAgentRequest) -> NewAgentExtras {
        let initial = request_provider(request);
        let mut provider_options: Vec<ProviderKind> = self
            .engine
            .config
            .providers
            .commands
            .keys()
            .map(|name| ProviderKind::from_str(name))
            .collect();
        if !provider_options.contains(&initial) {
            provider_options.insert(0, initial.clone());
        }
        let selected_provider = provider_options
            .iter()
            .position(|p| *p == initial)
            .unwrap_or(0);
        let draft = SessionSettings::default();
        let rules = self.collect_provider_watch_rule_summaries(&initial, &draft);
        NewAgentExtras {
            provider_options,
            selected_provider,
            initial_provider: Some(initial),
            show_advanced: false,
            draft,
            rules,
            hit_rows: Vec::new(),
        }
    }

    fn new_agent_extras_mut(&mut self) -> Option<(&mut NewAgentExtras, &mut NameNewAgentFocus)> {
        match &mut self.prompt {
            PromptState::NameNewAgent { extras, focus, .. } => Some((extras, focus)),
            _ => None,
        }
    }

    /// Space on the harness row: the next configured harness. Movement keys
    /// never change it (the movement tenet). Watch rules are per harness, so
    /// the rows and their draft arms are rebuilt for the new one.
    pub(crate) fn cycle_new_agent_provider(&mut self) {
        let Some((extras, focus)) = self.new_agent_extras_mut() else {
            return;
        };
        *focus = NameNewAgentFocus::Provider;
        if extras.provider_options.is_empty() {
            return;
        }
        extras.selected_provider = (extras.selected_provider + 1) % extras.provider_options.len();
        extras.draft.watch_rule_arm.clear();
        let provider = extras.provider_options[extras.selected_provider].clone();
        let draft = extras.draft.clone();
        let rules = self.collect_provider_watch_rule_summaries(&provider, &draft);
        if let Some((extras, _)) = self.new_agent_extras_mut() {
            extras.rules = rules;
        }
    }

    /// Space or a click on a harness, Advanced or setting row.
    pub(crate) fn activate_new_agent_row(&mut self, target: NameNewAgentFocus) {
        match target {
            NameNewAgentFocus::Provider => self.cycle_new_agent_provider(),
            NameNewAgentFocus::AdvancedToggle => {
                if let Some((extras, focus)) = self.new_agent_extras_mut() {
                    *focus = NameNewAgentFocus::AdvancedToggle;
                    extras.show_advanced = !extras.show_advanced;
                }
            }
            NameNewAgentFocus::Setting(row) => {
                if let Some((extras, focus)) = self.new_agent_extras_mut() {
                    *focus = NameNewAgentFocus::Setting(row);
                    apply_settings_toggle(&mut extras.draft, &mut extras.rules, row);
                }
            }
            NameNewAgentFocus::Input
            | NameNewAgentFocus::RandomizedNameCheckbox
            | NameNewAgentFocus::CopyChangesCheckbox => {}
        }
    }

    /// The row a press landed on, from the rects the last render published.
    pub(crate) fn new_agent_row_hit(&self, column: u16, row: u16) -> Option<NameNewAgentFocus> {
        let PromptState::NameNewAgent { extras, .. } = &self.prompt else {
            return None;
        };
        extras
            .hit_rows
            .iter()
            .find(|(rect, _)| super::input::contains_point(*rect, column, row))
            .map(|(_, focus)| *focus)
    }

    /// Draw the block into `area` and return each row's hit rect.
    pub(crate) fn render_new_agent_extras(
        &self,
        frame: &mut Frame,
        area: Rect,
        lines: &[ExtrasLine],
    ) -> Vec<(Rect, NameNewAgentFocus)> {
        let focus = match &self.prompt {
            PromptState::NameNewAgent { focus, .. } => *focus,
            _ => return Vec::new(),
        };
        let label = Style::default()
            .fg(self.theme.input_label_fg)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(self.theme.hint_dim_desc_fg);
        let text = Style::default().fg(self.theme.text_fg);
        let focused = self.theme.selection_style();
        let mut hits = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }
            let rect = Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1);
            let content = match line {
                ExtrasLine::Heading(title) => Line::from(Span::styled(format!(" {title}"), label)),
                ExtrasLine::Row {
                    target,
                    marker,
                    name,
                    desc,
                } => {
                    hits.push((rect, *target));
                    let style = if *target == focus { focused } else { text };
                    let mut spans = vec![Span::styled(format!("  {marker} {name}"), style)];
                    if !desc.is_empty() {
                        spans.push(Span::styled(format!("  {desc}"), dim));
                    }
                    Line::from(spans)
                }
            };
            Paragraph::new(content).render(rect, frame.buffer_mut());
        }
        hits
    }

    /// At confirm: point the request at the chosen harness, and hold the
    /// settings draft (when it differs from the defaults) for the create it
    /// belongs to.
    pub(crate) fn take_new_agent_choices(
        &mut self,
        extras: &NewAgentExtras,
        request: &mut CreateAgentRequest,
    ) {
        if let Some(chosen) = extras.selected()
            && extras.initial_provider.as_ref() != Some(chosen)
        {
            set_request_provider(request, chosen.clone());
        }
        self.pending_new_agent_settings = (extras.draft != SessionSettings::default())
            .then(|| (CreateTag::of(request), extras.draft.clone()));
    }

    /// Arm the held draft for the create now dispatching, if it is that
    /// create's; drop it otherwise.
    pub(crate) fn arm_new_agent_settings(&mut self, request: &CreateAgentRequest) {
        self.armed_new_agent_settings = self
            .pending_new_agent_settings
            .take()
            .filter(|(tag, _)| *tag == CreateTag::of(request))
            .map(|(_, draft)| draft);
    }

    /// The create committed: apply the draft through the settings modal's
    /// persist-before-mutate command.
    ///
    /// INTEGRATION: spawn-time settings (YOLO, AMQ verification) apply from
    /// the agent's NEXT launch, because the first spawn already happened in the
    /// create worker before the session id existed. When the launch env reads
    /// Engine::session_settings (maple/seedling: agent_env::session_settings_env
    /// and SessionSettings::yolo_launch_args), thread this draft into the first
    /// launch instead.
    pub(crate) fn apply_armed_new_agent_settings(&mut self, session_id: &str) {
        let Some(settings) = self.armed_new_agent_settings.take() else {
            return;
        };
        let spawn_time_differs =
            settings.yolo_permissions || settings.verify_envelope_override.is_some();
        match self
            .engine
            .apply(dux_core::engine::Command::SetSessionSettings {
                session_id: session_id.to_string(),
                settings: Box::new(settings),
                title: None,
            }) {
            Ok(reaction) => {
                self.apply_reaction(reaction);
                if spawn_time_differs {
                    self.set_warning(
                        "Agent created. Its YOLO / AMQ verification settings apply from its \
                         next launch: reconnect it to use them now.",
                    );
                }
            }
            Err(err) => self.set_error(format!(
                "Agent created, but its settings could not be saved: {err:#}. Set them in \
                 Session Settings."
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use crate::app::{OverlayMouseLayout, PromptState};
    use dux_core::session_settings::ContextMode;
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// The new-agent modal for a fresh agent in the test project, the way
    /// the new-agent action opens it once a project is chosen.
    fn open(app: &mut App) {
        let request = CreateAgentRequest::NewProject {
            project: app.engine.projects[0].clone(),
            custom_name: None,
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };
        app.open_name_new_agent_prompt(request).unwrap();
    }

    fn extras(app: &App) -> &NewAgentExtras {
        match &app.prompt {
            PromptState::NameNewAgent { extras, .. } => extras,
            other => panic!("expected name-new-agent prompt, got {other:?}"),
        }
    }

    fn focus(app: &App) -> NameNewAgentFocus {
        match &app.prompt {
            PromptState::NameNewAgent { focus, .. } => *focus,
            other => panic!("expected name-new-agent prompt, got {other:?}"),
        }
    }

    fn with_rows(app: &mut App, rows: Vec<(Rect, NameNewAgentFocus)>) {
        app.overlay_layout.active = OverlayMouseLayout::NameNewAgent {
            input: Rect::new(10, 5, 30, 1),
            checkbox: None,
            copy_checkbox: None,
        };
        if let PromptState::NameNewAgent { extras, .. } = &mut app.prompt {
            extras.hit_rows = rows;
        }
    }

    #[test]
    fn name_prompt_mouse_toggles_advanced_settings_row() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        with_rows(
            &mut app,
            vec![(Rect::new(10, 9, 40, 1), NameNewAgentFocus::AdvancedToggle)],
        );

        app.handle_prompt_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 12, 9));

        assert!(extras(&app).show_advanced);
        assert_eq!(focus(&app), NameNewAgentFocus::AdvancedToggle);
    }

    #[test]
    fn name_prompt_mouse_toggles_watch_rule_row() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        if let PromptState::NameNewAgent { extras, .. } = &mut app.prompt {
            extras.show_advanced = true;
            extras.rules = vec![WatchRuleSummary {
                idx: 0,
                label: "rate limit".to_string(),
                armed: true,
            }];
        }
        let row = NameNewAgentFocus::Setting(SettingsFocus::WatchRule(0));
        with_rows(&mut app, vec![(Rect::new(10, 12, 40, 1), row)]);

        app.handle_prompt_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 12, 12));

        assert_eq!(focus(&app), row);
        assert!(!extras(&app).rules[0].armed, "the click disarmed the rule");
        assert_eq!(extras(&app).draft.watch_rule_arm.get(&0), Some(&false));
    }

    /// Space picks the next harness; the chosen one is what the create
    /// launches, and the registered project's default is untouched.
    #[test]
    fn provider_row_picks_the_harness_the_create_launches() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        let before = extras(&app).selected().cloned().unwrap();
        app.activate_new_agent_row(NameNewAgentFocus::Provider);
        let chosen = extras(&app).selected().cloned().unwrap();
        assert_ne!(chosen, before);

        let PromptState::NameNewAgent {
            mut request,
            extras,
            ..
        } = std::mem::replace(&mut app.prompt, PromptState::None)
        else {
            unreachable!()
        };
        app.take_new_agent_choices(&extras, &mut request);
        assert_eq!(request_provider(&request), chosen);
        assert_eq!(app.engine.projects[0].default_provider, before);
    }

    /// Mode, YOLO and verify rows reuse the settings modal's toggle, and the
    /// draft reaches the new session through SetSessionSettings.
    #[test]
    fn advanced_draft_is_applied_to_the_created_session() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        app.activate_new_agent_row(NameNewAgentFocus::Setting(SettingsFocus::ModeWorker));
        app.activate_new_agent_row(NameNewAgentFocus::Setting(SettingsFocus::AutoClearOnDone));
        assert_eq!(extras(&app).draft.mode, ContextMode::Worker);

        let PromptState::NameNewAgent {
            mut request,
            extras,
            ..
        } = std::mem::replace(&mut app.prompt, PromptState::None)
        else {
            unreachable!()
        };
        app.take_new_agent_choices(&extras, &mut request);
        app.arm_new_agent_settings(&request);

        // The create has committed: the row exists, as it does when
        // CreateCommitted fires.
        let session = app.engine.sessions[0].clone();
        let _ = app.engine.session_store.create_session(&session);
        let session_id = session.id;
        app.apply_armed_new_agent_settings(&session_id);
        assert!(
            !app.status.message().contains("could not be saved"),
            "{}",
            app.status.message()
        );
        let saved = app.engine.session_settings_or_default(&session_id);
        assert_eq!(saved.mode, ContextMode::Worker);
        assert!(saved.auto_clear_on_task_done);
        assert!(app.armed_new_agent_settings.is_none(), "spent once");
    }

    /// A draft held for one create never attaches itself to another.
    #[test]
    fn a_held_draft_only_arms_for_its_own_create() {
        let mut app = test_app(default_bindings());
        open(&mut app);
        app.activate_new_agent_row(NameNewAgentFocus::Setting(SettingsFocus::Yolo));
        let PromptState::NameNewAgent {
            mut request,
            extras,
            ..
        } = std::mem::replace(&mut app.prompt, PromptState::None)
        else {
            unreachable!()
        };
        crate::app::input::set_create_agent_request_custom_name(&mut request, "one".into());
        app.take_new_agent_choices(&extras, &mut request);

        let mut other = request.clone();
        crate::app::input::set_create_agent_request_custom_name(&mut other, "two".into());
        app.arm_new_agent_settings(&other);
        assert!(app.armed_new_agent_settings.is_none());
        assert!(
            app.pending_new_agent_settings.is_none(),
            "dropped, not kept"
        );
    }

    /// A real render publishes the rows the mouse tests click, and opening
    /// Advanced adds the settings rows, the watch rules among them.
    #[test]
    fn render_publishes_harness_advanced_and_settings_rows() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = test_app(default_bindings());
        open(&mut app);
        let mut terminal = Terminal::new(TestBackend::new(120, 60)).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
        let targets: Vec<_> = extras(&app).hit_rows.iter().map(|(_, t)| *t).collect();
        assert_eq!(
            targets,
            vec![
                NameNewAgentFocus::Provider,
                NameNewAgentFocus::AdvancedToggle
            ]
        );

        app.activate_new_agent_row(NameNewAgentFocus::AdvancedToggle);
        terminal.draw(|f| app.render(f)).unwrap();
        let targets: Vec<_> = extras(&app).hit_rows.iter().map(|(_, t)| *t).collect();
        assert!(targets.contains(&NameNewAgentFocus::Setting(SettingsFocus::ModeWorker)));
        assert!(targets.contains(&NameNewAgentFocus::Setting(SettingsFocus::VerifySkip)));

        // Clicking the rendered Worker row selects it.
        let (rect, _) = *extras(&app)
            .hit_rows
            .iter()
            .find(|(_, t)| *t == NameNewAgentFocus::Setting(SettingsFocus::ModeWorker))
            .unwrap();
        app.handle_prompt_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            rect.x + 2,
            rect.y,
        ));
        assert_eq!(extras(&app).draft.mode, ContextMode::Worker);
    }
}
