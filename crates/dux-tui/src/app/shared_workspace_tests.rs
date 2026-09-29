//! TUI half of the fork's shared main-workspace mode (Phases 4 and 6): the
//! create choice, the delete dialog, rename, the second-writer consent and
//! the live multi-writer badge. Fork test names are kept.

use super::test_support::{default_bindings, test_app};
use super::*;
use dux_core::config::{ProjectConfig, WorkspaceConfig, WorkspaceMode};

fn shared_config(default_mode: WorkspaceMode) -> Option<WorkspaceConfig> {
    Some(WorkspaceConfig {
        default_mode,
        auto_resume_shared: false,
    })
}

fn open_create_prompt(app: &mut App) {
    let project = app.selected_project().cloned().expect("selected project");
    app.begin_new_agent_for_project(project).unwrap();
}

fn make_session_shared(app: &mut App, id: &str) {
    let project_path = app.engine.projects[0].path.clone();
    let session = app
        .engine
        .sessions
        .iter_mut()
        .find(|s| s.id == id)
        .expect("session");
    session.shared_workspace = true;
    session
        .workspace
        .as_managed_mut()
        .expect("managed")
        .worktree_path = project_path;
}

fn make_live(app: &mut App, id: &str) {
    let slot = app
        .engine
        .sessions
        .iter()
        .find(|s| s.id == id)
        .expect("session")
        .slot_tab_id()
        .to_owned();
    let client = PtyClient::spawn_with_env("cat", &[], std::path::Path::new("."), 24, 80, 100, &[])
        .expect("spawn cat");
    app.engine
        .providers
        .insert(dux_core::ids::TabId::new(slot.as_str()), client);
}

fn screen_text(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

fn select(app: &mut App, id: &str) {
    app.rebuild_left_items();
    let index = app
        .engine
        .sessions
        .iter()
        .position(|s| s.id == id)
        .expect("session");
    app.selected_left = app
        .left_items()
        .iter()
        .position(|item| matches!(item, LeftItem::Session(i) if *i == index))
        .expect("session row");
}

fn add_second_shared_session(app: &mut App, id: &str) {
    let mut second = app.engine.sessions[0].clone();
    second.id = id.to_string();
    second.agent_handle = id.to_string();
    second.slot_tab_id = format!("{id}-slot");
    second.title = Some(id.to_string());
    app.engine.sessions.push(second);
}

#[test]
fn legacy_config_create_flow_remains_isolated_and_empty_by_default() {
    let mut app = test_app(default_bindings());
    app.engine.config.workspace = None;

    open_create_prompt(&mut app);

    assert!(
        !matches!(
            app.prompt,
            PromptState::NameNewAgent {
                request: CreateAgentRequest::SharedWorkspace { .. },
                ..
            }
        ),
        "a config without [workspace] never creates a shared agent"
    );
    assert_eq!(
        app.status.tone(),
        crate::statusline::StatusTone::Busy,
        "the isolated flow goes through the branch inspection"
    );
}

#[test]
fn shared_default_opens_shared_request_with_prefilled_handle() {
    let mut app = test_app(default_bindings());
    app.engine.config.workspace = shared_config(WorkspaceMode::Shared);
    app.engine
        .config
        .defaults
        .enable_randomized_pet_name_by_default = true;

    open_create_prompt(&mut app);

    match &app.prompt {
        PromptState::NameNewAgent {
            request,
            input,
            randomize_name,
            ..
        } => {
            assert!(matches!(
                request,
                CreateAgentRequest::SharedWorkspace { .. }
            ));
            assert!(!input.text.is_empty());
            assert!(dux_core::model::is_valid_agent_handle(
                &dux_core::model::normalize_agent_handle(&input.text)
            ));
            assert!(*randomize_name);
        }
        other => panic!("expected shared name prompt, got {other:?}"),
    }
}

#[test]
fn project_workspace_override_controls_create_mode() {
    for (global, project_mode, expect_shared) in [
        (WorkspaceMode::Worktree, WorkspaceMode::Shared, true),
        (WorkspaceMode::Shared, WorkspaceMode::Worktree, false),
    ] {
        let mut app = test_app(default_bindings());
        app.engine.config.workspace = shared_config(global);
        let project = app.engine.projects[0].clone();
        app.engine.config.projects.push(ProjectConfig {
            id: project.id.clone(),
            path: project.path.clone(),
            name: Some(project.name.clone()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            workspace_mode: Some(project_mode),
        });

        open_create_prompt(&mut app);

        let shared = matches!(
            app.prompt,
            PromptState::NameNewAgent {
                request: CreateAgentRequest::SharedWorkspace { .. },
                ..
            }
        );
        assert_eq!(
            shared, expect_shared,
            "global {global:?}, project {project_mode:?}"
        );
    }
}

#[test]
fn fork_stays_isolated_under_shared_project_default() {
    let mut app = test_app(default_bindings());
    app.engine.config.workspace = shared_config(WorkspaceMode::Shared);

    app.fork_selected_session().unwrap();

    assert!(matches!(
        app.prompt,
        PromptState::NameNewAgent {
            request: CreateAgentRequest::ForkSession { .. },
            ..
        }
    ));
}

#[test]
fn shared_delete_dialog_hides_worktree_checkbox() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = test_app(default_bindings());
    make_session_shared(&mut app, "session-1");
    app.rebuild_left_items();
    select(&mut app, "session-1");
    app.confirm_delete_selected_session()
        .expect("open delete dialog");

    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
    terminal
        .draw(|frame| app.render(frame))
        .expect("render frame");

    match &app.prompt {
        PromptState::ConfirmDeleteAgent {
            target,
            delete_worktree,
            ..
        } => {
            assert!(!delete_worktree);
            assert!(
                !target.offers_worktree_checkbox(),
                "a shared checkout is never offered for removal"
            );
        }
        other => panic!("expected delete-agent prompt, got {other:?}"),
    }
    let screen = screen_text(terminal.backend().buffer());
    assert!(
        screen.contains("Runs in the shared project"),
        "the dialog says so:\n{screen}"
    );
}

#[test]
fn shared_session_rejects_real_branch_rename() {
    let mut app = test_app(default_bindings());
    make_session_shared(&mut app, "session-1");
    let original_branch = app.engine.sessions[0].branch_name().map(str::to_string);
    let original_title = app.engine.sessions[0].title.clone();

    app.apply_rename_session("session-1", "display-title".to_string(), true);

    assert_eq!(
        app.engine.sessions[0].branch_name().map(str::to_string),
        original_branch
    );
    assert_eq!(app.engine.sessions[0].title, original_title);
    assert!(app.status.message().contains("never renames"));
}

#[test]
fn shared_session_rename_changes_only_display_title() {
    let mut app = test_app(default_bindings());
    make_session_shared(&mut app, "session-1");
    let original_branch = app.engine.sessions[0].branch_name().map(str::to_string);
    let original_handle = app.engine.sessions[0].agent_handle().to_string();

    app.apply_rename_session("session-1", "display-title".to_string(), false);

    assert_eq!(
        app.engine.sessions[0].title.as_deref(),
        Some("display-title")
    );
    assert_eq!(
        app.engine.sessions[0].branch_name().map(str::to_string),
        original_branch
    );
    assert_eq!(app.engine.sessions[0].agent_handle(), original_handle);
}

#[test]
fn second_shared_writer_requires_confirmation_for_create_and_reconnect() {
    let mut app = test_app(default_bindings());
    make_session_shared(&mut app, "session-1");
    app.engine.sessions[0].title = Some("first-writer".to_string());
    add_second_shared_session(&mut app, "waiting");
    make_live(&mut app, "session-1");
    let project = app.engine.projects[0].clone();

    app.dispatch_create_agent_request(
        CreateAgentRequest::SharedWorkspace {
            project,
            custom_name: Some("second-writer".to_string()),
        },
        "creating".to_string(),
    )
    .unwrap();
    assert!(matches!(
        &app.prompt,
        PromptState::ConfirmSharedWriter {
            existing_agent,
            action: SharedWriterAction::Create { .. },
            focus: ConfirmFocus::Cancel,
        } if existing_agent == "first-writer"
    ));
    assert!(
        !app.engine
            .is_in_flight(&dux_core::engine::InFlightKey::CreateAgent),
        "nothing starts before the user confirms"
    );
    app.resolve_confirm_shared_writer(false);
    assert!(matches!(app.prompt, PromptState::None));

    select(&mut app, "waiting");
    app.reconnect_selected_session(false).unwrap();
    assert!(matches!(
        &app.prompt,
        PromptState::ConfirmSharedWriter {
            action: SharedWriterAction::Reconnect { session_id, force: false, .. },
            focus: ConfirmFocus::Cancel,
            ..
        } if session_id == "waiting"
    ));
    assert!(!app.engine.session_has_live_provider("waiting"));
    app.engine.shutdown_ptys(std::time::Duration::ZERO);
}

#[test]
fn shared_writer_confirmation_space_activates_focused_cancel() {
    let mut app = test_app(default_bindings());
    app.prompt = PromptState::ConfirmSharedWriter {
        existing_agent: "other-agent".to_string(),
        action: SharedWriterAction::Reconnect {
            session_id: "session-1".to_string(),
            force: false,
            seek_fullscreen: false,
        },
        focus: ConfirmFocus::Cancel,
    };

    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(app.prompt, PromptState::None));
    assert!(!app.engine.session_has_live_provider("session-1"));
}

/// The live multi-writer badge renders in the header while two shared agents
/// write in one checkout, and each shared row wears the SHARED badge.
#[test]
fn live_multi_writer_badge_and_shared_row_badge_render() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut app = test_app(default_bindings());
    make_session_shared(&mut app, "session-1");
    add_second_shared_session(&mut app, "second");
    app.rebuild_left_items();
    let draw = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(200, 40)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        screen_text(terminal.backend().buffer())
    };
    let screen = draw(&mut app);
    assert!(!screen.contains("LIVE WRITERS"));
    assert!(screen.contains("SHARED"));

    make_live(&mut app, "session-1");
    make_live(&mut app, "second");
    let screen = draw(&mut app);
    assert!(screen.contains("CURRENT STORE ONLY"), "{screen}");
    app.engine.shutdown_ptys(std::time::Duration::ZERO);
}
