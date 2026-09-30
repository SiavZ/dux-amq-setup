//! Presizing background agent PTYs to the windowed pane.
//!
//! The render pass sizes only the PTY it shows. Every other agent keeps
//! whatever grid it last had (an older layout, or the fullscreen size it was
//! left at), so the first time it is shown the render resizes it, the kernel
//! sends SIGWINCH, and agents like Claude Code and Codex clear and reprint
//! their whole transcript. That repaint is what makes a swap feel heavy.
//!
//! This pass sizes those background PTYs ahead of time, to the grid the
//! windowed center pane will give them, once that pane has held still for a
//! moment. A swap then finds the child already at the pane's size and sends
//! nothing.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use super::pty_ownership::PtyDriver;
use super::render::minimized_agent_term_size;
use super::*;

/// How long the windowed center pane must keep one geometry before background
/// PTYs follow it, so dragging a window edge does not make every agent redraw
/// at every intermediate size.
const PRESIZE_SETTLE: Duration = Duration::from_secs(1);

/// At most this many background PTYs are resized per UI tick, so a window
/// resize does not make sixty agents redraw in the same instant.
const PRESIZE_PER_TICK: usize = 2;

/// With nothing left to resize, how often the pass looks again for a PTY that
/// drifted (a new agent, a PR banner appearing, a tab opened).
const PRESIZE_RESCAN: Duration = Duration::from_secs(1);

#[derive(Debug, Default)]
pub(crate) struct PresizeState {
    /// The center lane as last rendered with no fullscreen overlay up, and when
    /// it took that geometry.
    area: Option<(Rect, Instant)>,
    /// When the last pass that found nothing to do ran. `None` while work is
    /// pending, so the next tick continues immediately.
    idle_since: Option<Instant>,
}

impl App {
    /// Record the center lane a windowed (not fullscreen) render drew into.
    pub(crate) fn note_minimized_center_area(&mut self, area: Rect) {
        if self.presize.area.is_some_and(|(seen, _)| seen == area) {
            return;
        }
        self.presize.area = Some((area, Instant::now()));
        self.presize.idle_since = None;
    }

    /// Resize a bounded number of background agent PTYs to the grid the
    /// windowed pane will show them at. Never touches the selected agent's
    /// focused tab (the render pass owns that one and its resize dedupe), never
    /// runs while a fullscreen overlay is up, and never claims a PTY: while a
    /// background server is serving, only PTYs this surface already drives are
    /// resized. Returns how many PTYs were resized.
    pub(crate) fn presize_background_agent_ptys(&mut self, now: Instant) -> usize {
        if !matches!(self.fullscreen_overlay, FullscreenOverlay::None) {
            return 0;
        }
        let Some((area, since)) = self.presize.area else {
            return 0;
        };
        if now.saturating_duration_since(since) < PRESIZE_SETTLE {
            return 0;
        }
        if self
            .presize
            .idle_since
            .is_some_and(|idle| now.saturating_duration_since(idle) < PRESIZE_RESCAN)
        {
            return 0;
        }
        // An armed take-over is spent by the next render of the pane it is
        // about. The sizing chokepoint drops an arm for any other pty, so a
        // presize now would cancel the user's take-over.
        if self.pending_pty_takeover.is_some() {
            return 0;
        }

        let selected = self.selected_session().map(|s| s.id.clone());
        let selected_tab = selected.as_deref().map(|id| self.focused_tab_id(id));
        let is_input = matches!(
            (self.input_target, self.session_surface),
            (InputTarget::Agent, SessionSurface::Agent)
                | (InputTarget::Terminal, SessionSurface::Terminal)
        );
        let always_show = self.engine.config.ui.always_show_tab_strip;
        // The same seat the sizing chokepoint consults.
        let serving = self.pty_ownership().is_some();

        let mut tab_ids: Vec<&TabId> = self.engine.providers.keys().collect();
        tab_ids.sort();
        let mut due: Vec<(String, u16, u16)> = Vec::new();
        for tab_id in tab_ids {
            if due.len() == PRESIZE_PER_TICK {
                break;
            }
            let tab_id = tab_id.as_str();
            if selected_tab.as_deref() == Some(tab_id) {
                continue;
            }
            let Some(client) = self.engine.providers.get(TabIdRef::new(tab_id)) else {
                continue;
            };
            if client.is_exited() {
                continue;
            }
            let Some(session_id) = self.engine.session_id_for_tab(tab_id) else {
                continue;
            };
            // `render_center` hides the banner while typing into the selected
            // agent, so a sibling tab is shown without it too.
            let banner = self.engine.pr_statuses.contains_key(&session_id)
                && !(is_input && selected.as_deref() == Some(session_id.as_str()));
            let tab_count = self
                .engine
                .ordered_tab_ids_for_session(SessionIdRef::new(&session_id))
                .len();
            let Some((rows, cols)) =
                minimized_agent_term_size(area, banner, tab_count, always_show)
            else {
                continue;
            };
            if client.grid_size() == Some((rows, cols)) {
                continue;
            }
            // Drawing is not a claim, and neither is presizing: a pty another
            // device drives, or nobody does yet, keeps its grid.
            if serving && !matches!(self.pty_driver(tab_id), PtyDriver::Mine) {
                continue;
            }
            due.push((tab_id.to_string(), rows, cols));
        }

        self.presize.idle_since = due.is_empty().then_some(now);
        let mut resized = 0;
        for (tab_id, rows, cols) in due {
            if self.resize_pty_if_permitted(&tab_id, rows, cols) {
                resized += 1;
            }
        }
        resized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::background_server::tests::FakeCompanion;
    use crate::app::test_support::{default_bindings, test_app};
    use crate::model::SessionStatus;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Prints `WINCH` on every SIGWINCH it receives, so a test can count the
    /// resizes the child actually saw rather than the ones dux meant to send.
    /// `ready` marks the trap as installed: a SIGWINCH before it is lost.
    const WINCH_COUNTER: &str = "trap 'echo WINCH' WINCH; echo ready; while :; do sleep 0.02; done";

    fn spawn_counter() -> crate::pty::PtyClient {
        let client = crate::pty::PtyClient::spawn(
            "sh",
            &["-c".to_string(), WINCH_COUNTER.to_string()],
            std::path::Path::new("."),
            10,
            10,
            100,
        )
        .expect("spawn pty");
        for _ in 0..300 {
            if client.visible_text_excerpt(20).contains("ready") {
                return client;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the SIGWINCH counter never installed its trap");
    }

    /// Two live agents, `session-1` selected in the windowed center pane and
    /// `session-2` in the background, both started at a stale 10x10 grid.
    fn two_agents() -> App {
        let mut app = test_app(default_bindings());
        let mut second = app.engine.sessions[0].clone();
        second.id = "session-2".to_string();
        second.agent_handle = "session-2".to_string();
        second.slot_tab_id = "session-2-slot".to_string();
        app.engine.sessions.push(second);
        for id in ["session-1", "session-2"] {
            app.engine.mark_session_status(id, SessionStatus::Active);
            app.engine
                .providers
                .insert(TabId::new(format!("{id}-slot")), spawn_counter());
        }
        app.rebuild_left_items();
        select(&mut app, "session-1");
        app.focus = FocusPane::Center;
        app.center_mode = CenterMode::Agent;
        app.session_surface = SessionSurface::Agent;
        app
    }

    fn select(app: &mut App, session_id: &str) {
        let index = app
            .engine
            .sessions
            .iter()
            .position(|s| s.id == session_id)
            .expect("session exists");
        app.selected_left = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::Session(i) if *i == index))
            .expect("session row is listed");
    }

    fn render(app: &mut App) {
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render succeeds");
    }

    fn grid(app: &App, tab: &str) -> Option<(u16, u16)> {
        app.engine
            .providers
            .get(TabIdRef::new(tab))
            .and_then(|client| client.grid_size())
    }

    fn winches(app: &App, tab: &str) -> usize {
        app.engine
            .providers
            .get(TabIdRef::new(tab))
            .map(|client| client.visible_text_excerpt(200).matches("WINCH").count())
            .unwrap_or_default()
    }

    /// Wait until the child has reported `count` SIGWINCHes, then a little
    /// longer so a late extra one would also show up.
    fn settle_winches(app: &App, tab: &str, count: usize) -> usize {
        for _ in 0..300 {
            if winches(app, tab) >= count {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(150));
        winches(app, tab)
    }

    fn settled() -> Instant {
        Instant::now() + PRESIZE_SETTLE + Duration::from_millis(50)
    }

    /// The pure geometry agrees with what render actually measures, with and
    /// without the tab strip.
    #[test]
    fn the_presize_geometry_matches_the_rendered_terminal_rect() {
        let mut app = two_agents();
        for extra_tab in [false, true] {
            if extra_tab {
                app.engine.agent_tabs.insert(
                    TabId::new("tab-2"),
                    dux_core::model::AgentTab {
                        id: "tab-2".to_string(),
                        session_id: "session-1".to_string(),
                        provider: dux_core::model::ProviderKind::new("claude"),
                        sort_order: 1,
                        created_at: chrono::Utc::now(),
                    },
                );
            }
            render(&mut app);
            let (area, _) = app
                .presize
                .area
                .expect("a windowed render records its area");
            let rendered = app.mouse_layout.agent_term.expect("the terminal was drawn");
            let tabs = app.session_tab_ids("session-1").len();
            assert_eq!(
                minimized_agent_term_size(area, false, tabs, false),
                Some((rendered.height, rendered.width)),
                "tab strip shown: {extra_tab}"
            );
        }
    }

    /// The bug: a background agent at a stale size is resized on the swap that
    /// shows it, and the child repaints its whole transcript on that SIGWINCH.
    /// Presized once the pane settles, the swap then sends nothing.
    #[test]
    fn a_stale_background_agent_is_presized_and_the_swap_sends_no_sigwinch() {
        let mut app = two_agents();
        render(&mut app);
        let pane = grid(&app, "session-1-slot").expect("the shown agent is sized");
        assert_ne!(pane, (10, 10), "the render sized the shown agent");
        assert_eq!(grid(&app, "session-2-slot"), Some((10, 10)));
        let selected_dedupe = (app.last_pty_size, app.last_pty_resize_target.clone());

        // Not before the geometry has held still.
        assert_eq!(app.presize_background_agent_ptys(Instant::now()), 0);
        assert_eq!(grid(&app, "session-2-slot"), Some((10, 10)));

        assert_eq!(app.presize_background_agent_ptys(settled()), 1);
        assert_eq!(grid(&app, "session-2-slot"), Some(pane));
        assert_eq!(
            (app.last_pty_size, app.last_pty_resize_target.clone()),
            selected_dedupe,
            "the shown surface's resize dedupe is the render pass's, untouched"
        );
        let before_swap = settle_winches(&app, "session-2-slot", 1);
        assert_eq!(
            before_swap, 1,
            "the presize is the one resize the child sees"
        );

        select(&mut app, "session-2");
        render(&mut app);
        assert_eq!(grid(&app, "session-2-slot"), Some(pane));
        assert_eq!(
            settle_winches(&app, "session-2-slot", before_swap + 1),
            before_swap,
            "swapping onto a presized agent must not send it a SIGWINCH"
        );

        // Nothing left to do: a later pass resizes nothing.
        assert_eq!(app.presize_background_agent_ptys(settled()), 0);
    }

    /// Fullscreen is not the geometry a background agent will be shown at, and
    /// while it is up nothing is presized. Leaving it (by rendering windowed
    /// again) puts an agent left at the fullscreen size back on the pane size.
    #[test]
    fn nothing_is_presized_while_a_fullscreen_overlay_is_up() {
        let mut app = two_agents();
        render(&mut app);
        let pane = grid(&app, "session-1-slot").expect("sized");

        app.fullscreen_overlay = FullscreenOverlay::Agent;
        render(&mut app);
        let fullscreen = grid(&app, "session-1-slot").expect("sized");
        assert_ne!(fullscreen, pane, "fullscreen sizes the shown agent bigger");
        assert_eq!(app.presize_background_agent_ptys(settled()), 0);
        assert_eq!(grid(&app, "session-2-slot"), Some((10, 10)));

        // Leave fullscreen by moving to the other agent: the one left behind at
        // the fullscreen size is brought back to the pane.
        app.fullscreen_overlay = FullscreenOverlay::None;
        select(&mut app, "session-2");
        render(&mut app);
        assert_eq!(grid(&app, "session-2-slot"), Some(pane));
        assert_eq!(grid(&app, "session-1-slot"), Some(fullscreen));
        assert_eq!(app.presize_background_agent_ptys(settled()), 1);
        assert_eq!(grid(&app, "session-1-slot"), Some(pane));
    }

    /// Presizing is not a claim. While a web server is serving, a PTY another
    /// device drives or nobody drives keeps its grid; one this surface drives
    /// is presized like any other.
    #[test]
    fn a_serving_web_servers_foreign_or_free_pty_is_not_presized() {
        let mut app = two_agents();
        for n in [3, 4] {
            let mut extra = app.engine.sessions[0].clone();
            extra.id = format!("session-{n}");
            extra.agent_handle = extra.id.clone();
            extra.slot_tab_id = format!("session-{n}-slot");
            app.engine.sessions.push(extra);
            app.engine
                .mark_session_status(&format!("session-{n}"), SessionStatus::Active);
            app.engine
                .providers
                .insert(TabId::new(format!("session-{n}-slot")), spawn_counter());
        }
        app.rebuild_left_items();
        select(&mut app, "session-1");

        let (companion, _recorded, seat) = FakeCompanion::serving_with_ownership();
        app.companion = Some(companion);
        seat.owners
            .claim("session-1-slot", seat.conn_id)
            .expect("this surface drives the shown agent");
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim("session-2-slot", browser)
            .expect("a browser drives session-2");
        seat.owners
            .claim("session-3-slot", seat.conn_id)
            .expect("this surface drives session-3");

        render(&mut app);
        let pane = grid(&app, "session-1-slot").expect("sized");
        assert_ne!(pane, (10, 10));

        assert_eq!(app.presize_background_agent_ptys(settled()), 1);
        assert_eq!(
            grid(&app, "session-2-slot"),
            Some((10, 10)),
            "the browser's pty keeps the browser's grid"
        );
        assert_eq!(grid(&app, "session-3-slot"), Some(pane));
        assert_eq!(
            grid(&app, "session-4-slot"),
            Some((10, 10)),
            "a pty nobody drives is not claimed by presizing it"
        );
        assert_eq!(app.pty_driver("session-4-slot"), PtyDriver::Free);
        assert!(
            seat.owners.is_owner("session-2-slot", browser),
            "and presizing took nothing from the browser"
        );
    }

    /// A window resize with many agents does not resize them all at once.
    #[test]
    fn at_most_a_few_ptys_are_presized_per_tick() {
        let mut app = two_agents();
        for n in 3..=5 {
            let mut extra = app.engine.sessions[0].clone();
            extra.id = format!("session-{n}");
            extra.agent_handle = extra.id.clone();
            extra.slot_tab_id = format!("session-{n}-slot");
            app.engine.sessions.push(extra);
            app.engine
                .mark_session_status(&format!("session-{n}"), SessionStatus::Active);
            app.engine
                .providers
                .insert(TabId::new(format!("session-{n}-slot")), spawn_counter());
        }
        app.rebuild_left_items();
        select(&mut app, "session-1");
        render(&mut app);

        let now = settled();
        assert_eq!(app.presize_background_agent_ptys(now), PRESIZE_PER_TICK);
        assert_eq!(app.presize_background_agent_ptys(now), PRESIZE_PER_TICK);
        assert_eq!(app.presize_background_agent_ptys(now), 0);
    }
}
