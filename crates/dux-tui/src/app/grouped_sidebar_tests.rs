//! The grouped sidebar (`ui.sidebar_style = "grouped"`, the default): one
//! header per project with every agent in it underneath, so every harness that
//! shares one checkout is visible as a sibling. Pins the real-shaped layout the
//! upgrade took away (three shared-workspace harnesses in one repo plus another
//! project) and that `"flat"` still yields upstream's list unchanged.

use super::test_support::{default_bindings, test_app};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn project(id: &str, name: &str) -> Project {
    Project {
        id: id.to_string(),
        name: name.to_string(),
        path: format!("/tmp/{id}"),
        explicit_default_provider: None,
        default_provider: ProviderKind::from_str("claude"),
        leading_branch: Some("main".to_string()),
        auto_reopen_agents: None,
        startup_command: None,
        env: Default::default(),
        current_branch: "main".to_string(),
        branch_status: dux_core::model::ProjectBranchStatus::Unknown,
        path_missing: false,
        created_at: None,
    }
}

/// A shared-workspace agent the way the user's database holds one: running in
/// the project checkout on its branch, named after its own handle.
fn shared_agent(id: &str, project_id: &str, provider: &str, status: SessionStatus) -> AgentSession {
    let mut session = test_app_session(id, project_id);
    session.provider = ProviderKind::from_str(provider);
    session.shared_workspace = true;
    session.title = Some(id.to_string());
    session.status = status;
    if let Some(managed) = session.workspace.as_managed_mut() {
        managed.worktree_path = format!("/tmp/{project_id}");
        managed.branch_name = "main".to_string();
    }
    session
}

fn test_app_session(id: &str, project_id: &str) -> AgentSession {
    let now = Utc::now();
    AgentSession {
        id: id.to_string(),
        agent_handle: dux_core::model::normalize_agent_handle(id),
        shared_workspace: false,
        deleted_at: None,
        slot_tab_id: format!("{id}-slot"),
        provider: ProviderKind::from_str("codex"),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: true,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: project_id.to_string(),
            project_path: Some(format!("/tmp/{project_id}")),
            source_branch: "main".to_string(),
            branch_name: id.to_string(),
            initial_branch: id.to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: format!("/tmp/worktrees/{id}"),
        }),
    }
}

/// The user's shape: Jobzy-infra holds three harnesses sharing one checkout
/// (claude, codex, opencode; one of them detached), and a second project holds
/// one more agent. Interleaved in `engine.sessions` the way activity sorting
/// scattered them.
fn real_shaped() -> (Vec<Project>, Vec<AgentSession>) {
    let projects = vec![project("infra", "Jobzy-infra"), project("web", "Jobzy-web")];
    let sessions = vec![
        shared_agent("infra-claude", "infra", "claude", SessionStatus::Active),
        shared_agent("web-dev", "web", "claude", SessionStatus::Active),
        shared_agent("infra-engineer", "infra", "codex", SessionStatus::Active),
        shared_agent(
            "infra-opencode",
            "infra",
            "opencode",
            SessionStatus::Detached,
        ),
    ];
    (projects, sessions)
}

fn grouped_app() -> App {
    let mut app = test_app(default_bindings());
    let (projects, sessions) = real_shaped();
    app.engine.projects = projects;
    app.engine.sessions = sessions;
    app.engine.config.ui.sidebar_style = "grouped".to_string();
    app.rebuild_left_items();
    app
}

/// The items as readable labels: `#name` for a header, the session id for an
/// agent row, `~inactive` for the flat tail's toggle.
fn labels(app: &App) -> Vec<String> {
    app.left_items()
        .iter()
        .map(|item| match item {
            LeftItem::Group(g) => format!("#{}", app.left_group(*g).unwrap().name),
            LeftItem::Session(i) => app.engine.sessions[*i].id.clone(),
            LeftItem::InactiveToggle => "~inactive".to_string(),
        })
        .collect()
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

#[test]
fn grouped_is_the_default_and_unknown_values_degrade_to_it() {
    assert_eq!(
        dux_core::config::Config::default().ui.sidebar_style,
        "grouped"
    );
    assert_eq!(
        SidebarStyle::from_config_str("grouped"),
        SidebarStyle::Grouped
    );
    assert_eq!(SidebarStyle::from_config_str("flat"), SidebarStyle::Flat);
    assert_eq!(SidebarStyle::from_config_str("tree"), SidebarStyle::Grouped);
    assert_eq!(SidebarStyle::from_config_str(""), SidebarStyle::Grouped);
}

#[test]
fn grouped_mode_lists_every_shared_harness_under_its_project_header() {
    let app = grouped_app();
    assert_eq!(
        labels(&app),
        vec![
            "#Jobzy-infra",
            // Active ones first (engine order), then the project's detached one:
            // inactive agents stay under their own project.
            "infra-claude",
            "infra-engineer",
            "infra-opencode",
            "#Jobzy-web",
            "web-dev",
        ]
    );
    let infra = app.left_group(0).unwrap();
    assert_eq!(infra.agent_count, 3);
    assert!(!app.left_items().contains(&LeftItem::InactiveToggle));
}

#[test]
fn flat_mode_is_unchanged_by_the_grouped_default() {
    let mut app = grouped_app();
    app.engine.config.ui.sidebar_style = "flat".to_string();
    app.rebuild_left_items();
    let (_, sessions) = real_shaped();
    let expected = build_left_items(
        &sessions,
        app.inactive_collapsed,
        AgentSortMode::from_config_str(&app.engine.config.ui.agent_sort),
        &|_| false,
        &|_| true,
    );
    assert_eq!(app.left_items(), expected.as_slice());
    assert!(app.left_groups_cache.is_empty());
    assert!(
        app.left_items()
            .iter()
            .all(|item| !matches!(item, LeftItem::Group(_)))
    );
}

#[test]
fn toggling_a_project_folds_its_rows_and_keeps_the_cursor_on_its_header() {
    let mut app = grouped_app();
    // From an agent row: the toggle folds the agent's own project.
    app.selected_left = 2; // infra-engineer
    app.toggle_collapse_selected_project();
    assert_eq!(labels(&app), vec!["#Jobzy-infra", "#Jobzy-web", "web-dev"]);
    assert_eq!(app.selected_left, 0);
    assert!(app.left_group(0).unwrap().collapsed);
    // Enter on the header expands it again.
    app.focus = FocusPane::Left;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(labels(&app).len(), 6);
    assert_eq!(app.selected_left, 0);
}

#[test]
fn a_header_selects_its_project_and_no_agent() {
    let mut app = grouped_app();
    app.selected_left = 4; // #Jobzy-web
    assert_eq!(app.selected_project().map(|p| p.id.as_str()), Some("web"));
    assert!(app.selected_session().is_none());
}

#[test]
fn keyboard_navigation_walks_headers_and_rows() {
    let mut app = grouped_app();
    app.selected_left = 0;
    let mut seen = vec![app.selected_left];
    while let Some(next) = app.next_selectable_left_item_after(app.selected_left) {
        app.selected_left = next;
        seen.push(next);
    }
    assert_eq!(seen, vec![0, 1, 2, 3, 4, 5]);
}

#[test]
fn a_filter_drops_projects_with_no_hit_and_opens_collapsed_ones() {
    let mut app = grouped_app();
    app.collapsed_groups.insert("infra".to_string());
    app.open_agent_filter();
    for c in "opencode".chars() {
        app.agent_filter.as_mut().unwrap().insert_char(c);
    }
    app.rebuild_left_items();
    assert_eq!(labels(&app), vec!["#Jobzy-infra", "infra-opencode"]);
    // The header still counts the whole project.
    assert_eq!(app.left_group(0).unwrap().agent_count, 3);
    app.close_agent_filter();
    // The user's collapse is untouched by the filter.
    assert_eq!(labels(&app), vec!["#Jobzy-infra", "#Jobzy-web", "web-dev"]);
}

#[test]
fn a_drag_can_only_land_on_a_sibling_in_the_same_project() {
    let app = grouped_app();
    // infra-claude (1) and infra-engineer (2) share a project; web-dev (5) not.
    assert!(app.left_items_share_group(1, 2));
    assert!(!app.left_items_share_group(1, 5));
    assert!(app.is_reorderable_left_item(1));
    // The detached harness is in the derived inactive bucket.
    assert!(!app.is_reorderable_left_item(3));
    // Headers are never dragged.
    assert!(!app.is_reorderable_left_item(0));
}

#[test]
fn move_agent_up_swaps_with_the_previous_sibling_in_its_project() {
    let mut app = grouped_app();
    app.selected_left = 2; // infra-engineer, below infra-claude
    app.move_selected_agent(super::reorder::MoveDir::Up);
    assert_eq!(
        labels(&app),
        vec![
            "#Jobzy-infra",
            "infra-engineer",
            "infra-claude",
            "infra-opencode",
            "#Jobzy-web",
            "web-dev",
        ]
    );
    assert_eq!(
        app.selected_session().map(|s| s.id.as_str()),
        Some("infra-engineer")
    );
}

#[test]
fn a_click_on_a_row_selects_that_agent() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = grouped_app();
    let mut terminal = Terminal::new(TestBackend::new(120, 50)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    let list = app.mouse_layout.left_list;
    let row = app
        .mouse_layout
        .left_row_to_item
        .iter()
        .position(|&i| i == 2)
        .expect("infra-engineer is on screen");
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: list.x + 2,
        row: list.y + row as u16,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.selected_left, 2);
    assert_eq!(
        app.selected_session().map(|s| s.id.as_str()),
        Some("infra-engineer")
    );
}

#[test]
fn the_grouped_sidebar_renders_headers_with_counts_and_every_harness() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut app = grouped_app();
    app.left_width_pct = 40;
    let mut terminal = Terminal::new(TestBackend::new(120, 50)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    let screen = screen_text(terminal.backend().buffer());
    assert!(screen.contains("▾ Jobzy-infra (3)"), "{screen}");
    assert!(screen.contains("▾ Jobzy-web (1)"), "{screen}");
    for name in [
        "infra-claude",
        "infra-engineer",
        "infra-opencode",
        "web-dev",
    ] {
        assert!(screen.contains(name), "{name} missing:\n{screen}");
    }
}

#[test]
fn every_agent_row_names_its_harness_in_both_styles() {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    for style in ["grouped", "flat"] {
        let mut app = grouped_app();
        app.engine.config.ui.sidebar_style = style.to_string();
        app.inactive_collapsed = false;
        app.inactive_collapse_overridden = true;
        app.rebuild_left_items();
        app.left_width_pct = 40;
        let mut terminal = Terminal::new(TestBackend::new(120, 50)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        let screen = screen_text(terminal.backend().buffer());
        for row in [
            "infra-claude (claude)",
            "infra-engineer (codex)",
            "infra-opencode (opencode)",
            "web-dev (claude)",
        ] {
            assert!(screen.contains(row), "{style}: {row} missing:\n{screen}");
        }
    }
}

#[test]
fn a_pending_provider_swap_shows_both_harnesses() {
    let codex = ProviderKind::from_str("codex");
    let claude = ProviderKind::from_str("claude");
    assert_eq!(
        super::render::agent_row_provider_suffix(&codex, &codex),
        " (codex)"
    );
    assert_eq!(
        super::render::agent_row_provider_suffix(&claude, &codex),
        " (claude → codex)"
    );
}

fn press(app: &mut App, c: char) {
    app.focus = FocusPane::Left;
    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
        .unwrap();
}

fn selected_id(app: &App) -> Option<String> {
    app.selected_session().map(|s| s.id.clone())
}

#[test]
fn braces_cycle_the_harnesses_of_one_project_and_wrap() {
    let mut app = grouped_app();
    app.selected_left = 1; // infra-claude
    press(&mut app, '}');
    assert_eq!(selected_id(&app).as_deref(), Some("infra-engineer"));
    press(&mut app, '}');
    assert_eq!(selected_id(&app).as_deref(), Some("infra-opencode"));
    // Wraps inside the project, never onto web-dev.
    press(&mut app, '}');
    assert_eq!(selected_id(&app).as_deref(), Some("infra-claude"));
    press(&mut app, '{');
    assert_eq!(selected_id(&app).as_deref(), Some("infra-opencode"));
}

#[test]
fn braces_skip_other_projects_in_the_flat_list_too() {
    let mut app = grouped_app();
    app.engine.config.ui.sidebar_style = "flat".to_string();
    app.rebuild_left_items();
    // Flat active order: infra-claude, web-dev, infra-engineer.
    app.selected_left = 0;
    assert_eq!(selected_id(&app).as_deref(), Some("infra-claude"));
    press(&mut app, '}');
    assert_eq!(selected_id(&app).as_deref(), Some("infra-engineer"));
}

#[test]
fn a_brace_on_a_collapsed_header_opens_it_and_steps_in() {
    let mut app = grouped_app();
    app.collapsed_groups.insert("infra".to_string());
    app.rebuild_left_items();
    app.selected_left = 0;
    press(&mut app, '}');
    assert_eq!(selected_id(&app).as_deref(), Some("infra-claude"));
}

/// The user's real database holds two agents whose project was removed from
/// the config (Gallery2): the grouped sidebar must still list them, under an
/// orphan group after the real projects, never drop them silently.
#[test]
fn agents_whose_project_is_gone_still_show_under_an_orphan_group() {
    let mut app = grouped_app();
    app.engine.sessions.push(shared_agent(
        "gallery-claude",
        "gone-project",
        "claude",
        SessionStatus::Detached,
    ));
    app.engine.sessions.push(shared_agent(
        "gallery-codex",
        "gone-project",
        "codex",
        SessionStatus::Detached,
    ));
    app.rebuild_left_items();
    let labels = labels(&app);
    let header = labels
        .iter()
        .position(|l| l.starts_with('#') && l.contains("gone-pro"))
        .expect("an orphan header for the removed project");
    assert!(
        header > labels.iter().position(|l| l == "#Jobzy-web").unwrap(),
        "orphans come after the real projects: {labels:?}"
    );
    // Both removed-project agents sit under that header (order follows the
    // active sort, which is not what this test pins).
    let mut members = labels[header + 1..header + 3].to_vec();
    members.sort();
    assert_eq!(
        members,
        vec!["gallery-claude".to_string(), "gallery-codex".to_string()]
    );
}

/// A reload reopens on the agent that was selected, not on the first project
/// header with an empty agent pane (which read as "the screen was lost").
#[test]
fn a_reload_reopens_on_the_agent_that_was_selected() {
    let mut app = grouped_app();
    app.selected_left = 0;
    assert!(matches!(app.left_items()[0], LeftItem::Group(_)));
    app.reload_selected_session = Some("infra-opencode".to_string());
    app.restore_selection_after_reload();
    assert_eq!(selected_id(&app).as_deref(), Some("infra-opencode"));
    assert!(app.reload_selected_session.is_none(), "consumed once");
    // An agent that no longer exists leaves the cursor where it was.
    app.reload_selected_session = Some("gone".to_string());
    app.restore_selection_after_reload();
    assert_eq!(selected_id(&app).as_deref(), Some("infra-opencode"));
}

// ---- ui.sidebar_density = "compact" (the default) ----

fn compact_app(style: &str) -> App {
    let mut app = grouped_app();
    app.engine.config.ui.sidebar_style = style.to_string();
    app.engine.config.ui.sidebar_density = "compact".to_string();
    app.inactive_collapsed = false;
    app.inactive_collapse_overridden = true;
    app.rebuild_left_items();
    app.left_width_pct = 40;
    app
}

fn draw(app: &mut App) -> ratatui::buffer::Buffer {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let mut terminal = Terminal::new(TestBackend::new(120, 50)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    terminal.backend().buffer().clone()
}

/// The agent list's visible rows as text (gutters stripped, right-trimmed),
/// one per screen row of the list content, up to the first blank row.
fn sidebar_rows(app: &App, buffer: &ratatui::buffer::Buffer) -> Vec<String> {
    let list = app.mouse_layout.left_list;
    (list.y..list.y + list.height)
        .map(|y| {
            let mut row = String::new();
            for x in list.x + 1..list.x + list.width.saturating_sub(1) {
                row.push_str(buffer[(x, y)].symbol());
            }
            row.trim_end().to_string()
        })
        .take_while(|row| !row.is_empty())
        .collect()
}

#[test]
fn compact_is_the_default_density_and_unknown_values_degrade_to_it() {
    assert_eq!(
        dux_core::config::Config::default().ui.sidebar_density,
        "compact"
    );
    assert_eq!(
        SidebarDensity::from_config_str("comfortable"),
        SidebarDensity::Comfortable
    );
    assert_eq!(
        SidebarDensity::from_config_str("compact"),
        SidebarDensity::Compact
    );
    assert_eq!(
        SidebarDensity::from_config_str("dense"),
        SidebarDensity::Compact
    );
    assert_eq!(SidebarDensity::from_config_str(""), SidebarDensity::Compact);
    let mut app = compact_app("grouped");
    app.engine.config.ui.sidebar_density = "dense".to_string();
    assert_eq!(app.sidebar_density(), SidebarDensity::Compact);
}

#[test]
fn compact_grouped_renders_one_line_per_header_and_agent_with_tree_connectors() {
    let mut app = compact_app("grouped");
    let buffer = draw(&mut app);
    let rows = sidebar_rows(&app, &buffer);
    assert_eq!(rows.len(), 6, "{rows:#?}");
    let expected: [(&str, bool); 6] = [
        ("▾ Jobzy-infra (3)", false),
        ("├ ● infra-claude (claude)", true),
        ("├ ● infra-engineer (codex)", true),
        ("└ ◎ infra-opencode (opencode)", true),
        ("▾ Jobzy-web (1)", false),
        ("└ ● web-dev (claude)", true),
    ];
    for (row, (left, shared)) in rows.iter().zip(expected) {
        assert!(row.starts_with(left), "{row:?} should start {left:?}");
        if shared {
            // The SHARED badge is pinned to the right edge, past a gap.
            assert!(row.ends_with("SHARED"), "{row:?}");
            let gap = &row[left.len()..row.len() - "SHARED".len()];
            assert!(gap.len() >= 2 && gap.trim().is_empty(), "{row:?}");
        } else {
            assert_eq!(row, left);
        }
    }
}

#[test]
fn compact_heights_are_all_one_and_clicks_map_row_n_to_item_n() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = compact_app("grouped");
    let row_width = 40;
    let (_, heights) = app.agent_sidebar_items(app.left_items(), true, row_width);
    assert_eq!(heights, vec![1u16; app.left_items().len()]);
    draw(&mut app);
    let map = app.mouse_layout.left_row_to_item.clone();
    assert_eq!(&map[..6], &[0, 1, 2, 3, 4, 5]);
    for n in [5usize, 3, 1, 4] {
        let list = app.mouse_layout.left_list;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: list.x + 2,
            row: list.y + n as u16,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: list.x + 2,
            row: list.y + n as u16,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.selected_left, n, "click on row {n}");
        draw(&mut app);
    }
}

#[test]
fn compact_selection_tints_exactly_one_row_for_agents_and_headers() {
    for selected in [2usize, 4] {
        let mut app = compact_app("grouped");
        app.focus = FocusPane::Left;
        app.left_section = LeftSection::Projects;
        app.selected_left = selected;
        let buffer = draw(&mut app);
        let list = app.mouse_layout.left_list;
        let tint = app.theme.selection_bar_tint();
        let tinted: Vec<u16> = (list.y..list.y + list.height)
            .filter(|&y| (list.x..list.x + list.width).all(|x| buffer[(x, y)].bg == tint))
            .collect();
        assert_eq!(tinted, vec![list.y + selected as u16], "item {selected}");
        // No half-cell frame edges anywhere in the list.
        for y in list.y..list.y + list.height {
            for x in list.x..list.x + list.width {
                let s = buffer[(x, y)].symbol();
                assert!(s != "▄" && s != "▀", "frame edge at {x},{y}");
            }
        }
    }
}

#[test]
fn compact_list_reserves_no_top_margin_row() {
    let mut app = compact_app("grouped");
    let buffer = draw(&mut app);
    let rows = sidebar_rows(&app, &buffer);
    assert_eq!(rows.first().map(String::as_str), Some("▾ Jobzy-infra (3)"));
    // The list starts on the block's first inner row.
    let list = app.mouse_layout.left_list;
    let above: String = (list.x..list.x + list.width)
        .map(|x| buffer[(x, list.y - 1)].symbol().to_string())
        .collect();
    assert!(
        above.contains("Agents"),
        "the title border sits right above: {above:?}"
    );
}

#[test]
fn compact_flat_rows_name_harness_and_project_on_one_line() {
    let mut app = compact_app("flat");
    let buffer = draw(&mut app);
    let rows = sidebar_rows(&app, &buffer);
    let find = |needle: &str| {
        rows.iter()
            .find(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle} missing: {rows:#?}"))
            .clone()
    };
    assert!(find("infra-claude (claude)").contains("※ Jobzy-infra"));
    assert!(find("web-dev (claude)").contains("※ Jobzy-web"));
    assert!(find("infra-opencode (opencode)").contains("※ Jobzy-infra"));
    // No tree connectors in flat style, and no headers.
    assert!(
        rows.iter()
            .all(|r| !r.starts_with('├') && !r.starts_with('└'))
    );
    assert!(rows.iter().any(|r| r.starts_with("▾ Inactive (1)")));
    let (_, heights) = app.agent_sidebar_items(app.left_items(), true, 40);
    assert!(heights.iter().all(|&h| h == 1));
}

#[test]
fn comfortable_keeps_two_line_rows_with_spacers() {
    let mut app = compact_app("grouped");
    app.engine.config.ui.sidebar_density = "comfortable".to_string();
    let (_, heights) = app.agent_sidebar_items(app.left_items(), true, 40);
    assert_eq!(heights, vec![2, 3, 3, 3, 3, 3]);
    let buffer = draw(&mut app);
    let screen = screen_text(&buffer);
    assert!(!screen.contains("├ "), "{screen}");
    assert!(screen.contains("infra-claude (claude)"), "{screen}");
}

#[test]
fn compact_long_name_drops_the_badge_before_cutting_the_name() {
    let mut app = compact_app("grouped");
    // Fits beside the connector, glyph and harness, but not with SHARED too.
    app.engine.sessions[0].title = Some("a-rather-long-agent-name-here".to_string());
    let buffer = draw(&mut app);
    let rows = sidebar_rows(&app, &buffer);
    let row = rows
        .iter()
        .find(|r| r.contains("a-rather-long"))
        .expect("row");
    assert_eq!(row, "├ ● a-rather-long-agent-name-here (claude)");
}

#[test]
fn compact_filter_highlights_the_name() {
    let mut app = compact_app("grouped");
    app.open_agent_filter();
    for c in "engineer".chars() {
        app.agent_filter.as_mut().unwrap().insert_char(c);
    }
    app.rebuild_left_items();
    let buffer = draw(&mut app);
    let list = app.mouse_layout.left_list;
    let hit = app.theme.search_match_fg;
    let found = (list.y..list.y + list.height).any(|y| {
        (list.x..list.x + list.width)
            .any(|x| buffer[(x, y)].symbol() == "e" && buffer[(x, y)].fg == hit)
    });
    assert!(found, "{}", screen_text(&buffer));
}

#[test]
fn the_config_documents_sidebar_density() {
    let rendered = crate::config::render_default_config();
    assert!(
        rendered.contains("sidebar_density = \"compact\""),
        "{rendered}"
    );
    assert!(rendered.contains("# How much room each row of the TUI's agent list takes."));
    assert!(rendered.contains("#                   ├ ◐ backend (codex)"));
    assert!(rendered.contains("# two-line rows. Unknown values fall back to \"compact\"."));
}
