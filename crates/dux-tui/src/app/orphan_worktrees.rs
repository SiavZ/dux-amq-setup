//! The TUI half of the opt-in orphan-worktree cleaner (fork 5c9edb78): the
//! `prune-orphan-worktrees` palette command.
//!
//! Every decision lives in [`dux_core::orphan_worktrees`], which fails closed and
//! re-inventories before each removal. This file only opens the listing, walks
//! the user through a per-item confirmation that shows the dirty state and keeps
//! the branch unless the user opts in, and runs the git work off the UI thread.
//!
//! One modal, two stages ([`OrphanWorktreesStage`]), so there is one
//! `PromptState` variant for the exhaustive registries rather than two.

use std::sync::mpsc;

use dux_core::orphan_worktrees::OrphanWorktreeCandidate;

use super::*;

/// Where the cleaner modal is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OrphanWorktreesStage {
    /// The inventory worker has not answered yet.
    Loading,
    /// Picking a row.
    List,
    /// Confirming the selected row. `delete_branch` starts false: the branch
    /// is kept unless the user asks, per item.
    Confirm { delete_branch: bool },
    /// A removal is running for the selected row.
    Removing,
}

#[derive(Clone, Debug)]
pub(crate) struct OrphanWorktreesPrompt {
    pub(crate) candidates: Vec<OrphanWorktreeCandidate>,
    pub(crate) selected: usize,
    pub(crate) stage: OrphanWorktreesStage,
}

/// What a cleaner worker reports back through `App::orphan_worktrees_rx`.
pub(crate) enum OrphanWorktreesAnswer {
    Inventory(Result<Vec<OrphanWorktreeCandidate>, String>),
    Removed {
        worktree_path: PathBuf,
        delete_branch: bool,
        result: Result<(), String>,
    },
}

impl App {
    /// `prune-orphan-worktrees`: open the cleaner and start the inventory.
    pub(crate) fn open_orphan_worktree_cleaner(&mut self) -> Result<()> {
        if self.orphan_worktrees_rx.is_some() {
            self.set_warning("Orphan worktree cleanup is already running.");
            return Ok(());
        }
        self.prompt = PromptState::OrphanWorktrees(OrphanWorktreesPrompt {
            candidates: Vec::new(),
            selected: 0,
            stage: OrphanWorktreesStage::Loading,
        });
        let paths = self.engine.paths.clone();
        self.spawn_orphan_worker("orphan-worktree-inventory", move || {
            OrphanWorktreesAnswer::Inventory(
                dux_core::orphan_worktrees::inventory(&paths).map_err(|err| format!("{err:#}")),
            )
        });
        Ok(())
    }

    fn spawn_orphan_worker(
        &mut self,
        name: &str,
        work: impl FnOnce() -> OrphanWorktreesAnswer + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        self.orphan_worktrees_rx = Some(rx);
        let spawned = thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                let _ = tx.send(work());
            });
        if let Err(err) = spawned {
            self.orphan_worktrees_rx = None;
            self.prompt = PromptState::None;
            self.set_error(format!("Could not start the orphan worktree worker: {err}"));
        }
    }

    /// Fold a finished cleaner worker into the modal. Called every tick.
    pub(crate) fn drain_orphan_worktrees(&mut self) {
        let Some(rx) = self.orphan_worktrees_rx.as_ref() else {
            return;
        };
        let answer = match rx.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.orphan_worktrees_rx = None;
                if matches!(self.prompt, PromptState::OrphanWorktrees(_)) {
                    self.prompt = PromptState::None;
                }
                self.set_error("The orphan worktree worker stopped without an answer.");
                return;
            }
        };
        self.orphan_worktrees_rx = None;
        self.mark_frame_dirty();
        self.apply_orphan_worktrees_answer(answer);
    }

    pub(crate) fn apply_orphan_worktrees_answer(&mut self, answer: OrphanWorktreesAnswer) {
        match answer {
            OrphanWorktreesAnswer::Inventory(Ok(candidates)) => {
                if candidates.is_empty() {
                    self.close_orphan_prompt();
                    self.set_info("No eligible orphan worktrees found.");
                    return;
                }
                if let PromptState::OrphanWorktrees(prompt) = &mut self.prompt {
                    prompt.candidates = candidates;
                    prompt.selected = 0;
                    prompt.stage = OrphanWorktreesStage::List;
                    self.set_info(
                        "Orphan worktree inventory loaded; review each item before removal.",
                    );
                }
            }
            OrphanWorktreesAnswer::Inventory(Err(message)) => {
                self.close_orphan_prompt();
                self.set_error(dux_core::sanitize::for_terminal(&message));
            }
            OrphanWorktreesAnswer::Removed {
                worktree_path,
                delete_branch,
                result,
            } => {
                let shown = dux_core::sanitize::for_terminal(&worktree_path.display().to_string());
                let PromptState::OrphanWorktrees(prompt) = &mut self.prompt else {
                    // The modal was closed while git ran: still say what happened.
                    match result {
                        Ok(()) => self.set_info(format!("Removed orphan worktree {shown}.")),
                        Err(message) => self.set_error(dux_core::sanitize::for_terminal(&message)),
                    }
                    return;
                };
                prompt.stage = OrphanWorktreesStage::List;
                match result {
                    Ok(()) => {
                        prompt
                            .candidates
                            .retain(|item| item.worktree_path != worktree_path);
                        prompt.selected = prompt
                            .selected
                            .min(prompt.candidates.len().saturating_sub(1));
                        if prompt.candidates.is_empty() {
                            self.close_orphan_prompt();
                        }
                        let branch_note = if delete_branch {
                            " and deleted its branch"
                        } else {
                            "; its branch was kept"
                        };
                        self.set_info(format!("Removed orphan worktree {shown}{branch_note}."));
                    }
                    Err(message) => self.set_error(dux_core::sanitize::for_terminal(&message)),
                }
            }
        }
    }

    fn close_orphan_prompt(&mut self) {
        if matches!(self.prompt, PromptState::OrphanWorktrees(_)) {
            self.prompt = PromptState::None;
        }
    }

    /// Esc from the cleaner: a confirmation steps back to the list, anything
    /// else closes. A running removal keeps running and reports when done.
    pub(crate) fn cancel_orphan_worktrees_prompt(&mut self) {
        if let PromptState::OrphanWorktrees(prompt) = &mut self.prompt
            && matches!(prompt.stage, OrphanWorktreesStage::Confirm { .. })
        {
            prompt.stage = OrphanWorktreesStage::List;
            return;
        }
        self.prompt = PromptState::None;
    }

    pub(crate) fn handle_orphan_worktrees_prompt_key(&mut self, key: KeyEvent) -> Option<bool> {
        let PromptState::OrphanWorktrees(prompt) = &mut self.prompt else {
            return None;
        };
        let scope = match prompt.stage {
            OrphanWorktreesStage::Confirm { .. } => BindingScope::Dialog,
            _ => BindingScope::Palette,
        };
        let action = self.bindings.lookup(&key, scope);
        if matches!(action, Some(Action::CloseOverlay)) || key.code == KeyCode::Esc {
            self.cancel_orphan_worktrees_prompt();
            return Some(false);
        }
        let PromptState::OrphanWorktrees(prompt) = &mut self.prompt else {
            return Some(false);
        };
        match prompt.stage.clone() {
            OrphanWorktreesStage::Loading | OrphanWorktreesStage::Removing => {}
            OrphanWorktreesStage::List => match action {
                Some(Action::MoveDown) if prompt.selected + 1 < prompt.candidates.len() => {
                    prompt.selected += 1;
                }
                Some(Action::MoveUp) if prompt.selected > 0 => prompt.selected -= 1,
                Some(Action::Confirm) if !prompt.candidates.is_empty() => {
                    prompt.stage = OrphanWorktreesStage::Confirm {
                        delete_branch: false,
                    };
                }
                _ => {}
            },
            OrphanWorktreesStage::Confirm { delete_branch } => {
                let has_branch = prompt
                    .candidates
                    .get(prompt.selected)
                    .is_some_and(|candidate| candidate.branch.is_some());
                match key.code {
                    // `b` toggles branch deletion, only when there is a branch.
                    KeyCode::Char('b') | KeyCode::Char(' ') if has_branch => {
                        prompt.stage = OrphanWorktreesStage::Confirm {
                            delete_branch: !delete_branch,
                        };
                    }
                    _ if matches!(action, Some(Action::Confirm)) => {
                        self.dispatch_orphan_worktree_removal(delete_branch && has_branch);
                    }
                    _ => {}
                }
            }
        }
        Some(false)
    }

    fn dispatch_orphan_worktree_removal(&mut self, delete_branch: bool) {
        let PromptState::OrphanWorktrees(prompt) = &mut self.prompt else {
            return;
        };
        let Some(candidate) = prompt.candidates.get(prompt.selected).cloned() else {
            prompt.stage = OrphanWorktreesStage::List;
            return;
        };
        prompt.stage = OrphanWorktreesStage::Removing;
        let paths = self.engine.paths.clone();
        self.spawn_orphan_worker("orphan-worktree-remove", move || {
            let result =
                dux_core::orphan_worktrees::remove(&paths, &candidate.worktree_path, delete_branch)
                    .map_err(|err| format!("{err:#}"));
            OrphanWorktreesAnswer::Removed {
                worktree_path: candidate.worktree_path,
                delete_branch,
                result,
            }
        });
    }

    pub(crate) fn render_orphan_worktrees_prompt(&mut self, frame: &mut Frame) {
        let PromptState::OrphanWorktrees(prompt) = &self.prompt else {
            return;
        };
        self.render_dim_overlay(frame);
        let area = render::centered_rect(88, 72, frame.area());
        self.clear_overlay_area(frame, area);
        let outer = self.themed_overlay_block("Orphan Worktree Cleaner");
        let inner = outer.inner(area);
        outer.render(area, frame.buffer_mut());
        let [intro_area, list_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4),
                Constraint::Min(3),
                Constraint::Length(2),
            ])
            .areas(inner);
        let dim = Style::default().fg(self.theme.hint_desc_fg);

        let intro = match &prompt.stage {
            OrphanWorktreesStage::Loading => vec![Line::from(
                " Listing Git-registered worktrees under dux's root that no agent owns...",
            )],
            OrphanWorktreesStage::Confirm { delete_branch } => {
                let candidate = prompt.candidates.get(prompt.selected);
                let branch = candidate.and_then(|c| c.branch.as_deref());
                let mut lines = vec![Line::from(Span::styled(
                    format!(
                        " Remove {}?",
                        dux_core::sanitize::for_terminal(
                            &candidate
                                .map(|c| c.worktree_path.display().to_string())
                                .unwrap_or_default()
                        )
                    ),
                    Style::default().add_modifier(Modifier::BOLD),
                ))];
                if candidate.is_some_and(|c| c.dirty) {
                    lines.push(Line::from(Span::styled(
                        " It has uncommitted or untracked work, which will be deleted.",
                        Style::default()
                            .fg(self.theme.warning_fg)
                            .add_modifier(Modifier::BOLD),
                    )));
                }
                lines.push(Line::from(Span::styled(
                    match (branch, delete_branch) {
                        (None, _) => {
                            " Detached HEAD: there is no branch to keep or delete.".to_string()
                        }
                        (Some(b), false) => format!(
                            " [ ] also delete branch {} (b to toggle; kept by default)",
                            dux_core::sanitize::for_terminal(b)
                        ),
                        (Some(b), true) => format!(
                            " [x] also delete branch {} (b to toggle)",
                            dux_core::sanitize::for_terminal(b)
                        ),
                    },
                    dim,
                )));
                lines
            }
            _ => vec![
                Line::from(format!(
                    " {} Git-registered worktree{} under dux's root ha{} no agent, live or deleted.",
                    prompt.candidates.len(),
                    if prompt.candidates.len() == 1 {
                        ""
                    } else {
                        "s"
                    },
                    if prompt.candidates.len() == 1 {
                        "s"
                    } else {
                        "ve"
                    },
                )),
                Line::from(Span::styled(
                    " Nothing is removed until you confirm an individual item.",
                    dim,
                )),
                Line::from(Span::styled(" Branches are kept by default.", dim)),
            ],
        };
        Paragraph::new(intro).render(intro_area, frame.buffer_mut());

        let items = prompt
            .candidates
            .iter()
            .map(|candidate| {
                let (dirty, dirty_style) = if candidate.dirty {
                    (
                        "DIRTY",
                        Style::default()
                            .fg(self.theme.warning_fg)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("clean", Style::default().fg(self.theme.session_detached))
                };
                let branch = dux_core::sanitize::for_terminal(
                    candidate.branch.as_deref().unwrap_or("detached HEAD"),
                );
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {dirty:<5} "), dirty_style),
                    Span::styled(
                        format!("{branch}  "),
                        Style::default().fg(self.theme.branch_fg),
                    ),
                    Span::raw(dux_core::sanitize::for_terminal(
                        &candidate.worktree_path.display().to_string(),
                    )),
                ]))
            })
            .collect::<Vec<_>>();
        let mut state = ListState::default()
            .with_selected((!prompt.candidates.is_empty()).then_some(prompt.selected));
        StatefulWidget::render(
            List::new(items)
                .highlight_symbol("› ")
                .highlight_style(self.theme.selection_style()),
            list_area,
            frame.buffer_mut(),
            &mut state,
        );

        let confirm_key = self.bindings.label_for(Action::Confirm);
        let close_key = self.bindings.label_for(Action::CloseOverlay);
        let mut hint = vec![Span::raw(" ")];
        match prompt.stage {
            OrphanWorktreesStage::Confirm { .. } => {
                hint.extend(self.theme.key_badge_default(&confirm_key));
                hint.push(Span::styled(" remove  ", dim));
                hint.extend(self.theme.key_badge_default(&close_key));
                hint.push(Span::styled(" back", dim));
            }
            OrphanWorktreesStage::Removing => {
                hint.push(Span::styled("Removing...", dim));
            }
            _ => {
                hint.extend(self.theme.key_badge_default(&confirm_key));
                hint.push(Span::styled(" review  ", dim));
                hint.extend(self.theme.key_badge_default(&close_key));
                hint.push(Span::styled(" close", dim));
            }
        }
        Paragraph::new(Line::from(hint)).render(hint_area, frame.buffer_mut());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn candidate(name: &str, branch: Option<&str>, dirty: bool) -> OrphanWorktreeCandidate {
        OrphanWorktreeCandidate {
            project_path: PathBuf::from("/tmp/repo"),
            worktree_path: PathBuf::from(format!("/tmp/dux/worktrees/{name}")),
            branch: branch.map(str::to_string),
            dirty,
        }
    }

    fn open_list(app: &mut App, candidates: Vec<OrphanWorktreeCandidate>) {
        app.prompt = PromptState::OrphanWorktrees(OrphanWorktreesPrompt {
            candidates,
            selected: 0,
            stage: OrphanWorktreesStage::List,
        });
    }

    fn stage(app: &App) -> OrphanWorktreesStage {
        match &app.prompt {
            PromptState::OrphanWorktrees(prompt) => prompt.stage.clone(),
            other => panic!("cleaner closed: {other:?}"),
        }
    }

    /// Enter on a row only opens its confirmation, with branch deletion OFF;
    /// Esc steps back to the list without removing anything.
    #[test]
    fn confirming_a_row_starts_with_branch_kept_and_esc_steps_back() {
        let mut app = test_app(default_bindings());
        open_list(&mut app, vec![candidate("a", Some("a"), true)]);

        app.handle_orphan_worktrees_prompt_key(key(KeyCode::Enter));
        assert_eq!(
            stage(&app),
            OrphanWorktreesStage::Confirm {
                delete_branch: false
            }
        );
        app.handle_orphan_worktrees_prompt_key(key(KeyCode::Char('b')));
        assert_eq!(
            stage(&app),
            OrphanWorktreesStage::Confirm {
                delete_branch: true
            }
        );
        app.handle_orphan_worktrees_prompt_key(key(KeyCode::Esc));
        assert_eq!(stage(&app), OrphanWorktreesStage::List);
        assert!(app.orphan_worktrees_rx.is_none(), "nothing was dispatched");

        app.handle_orphan_worktrees_prompt_key(key(KeyCode::Esc));
        assert!(matches!(app.prompt, PromptState::None));
    }

    /// Fork test, adapted to the one-variant modal: Enter on a row only opens
    /// its confirmation with branch deletion OFF, Esc backs out without
    /// starting a worker, `b` opts into branch deletion, and a second Enter is
    /// what dispatches. Driven through the real `handle_key` path.
    #[test]
    fn orphan_cleaner_requires_per_item_confirmation_and_preserves_branch_by_default() {
        let mut app = test_app(default_bindings());
        let paths = app.engine.paths.clone();
        app.prompt = PromptState::OrphanWorktrees(OrphanWorktreesPrompt {
            candidates: vec![OrphanWorktreeCandidate {
                project_path: paths.root.clone(),
                worktree_path: paths.worktrees_root.join("orphan"),
                branch: Some("keep-by-default".to_string()),
                dirty: true,
            }],
            selected: 0,
            stage: OrphanWorktreesStage::List,
        });

        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(
            stage(&app),
            OrphanWorktreesStage::Confirm {
                delete_branch: false
            }
        );
        app.handle_key(key(KeyCode::Esc)).unwrap();
        assert_eq!(stage(&app), OrphanWorktreesStage::List);
        assert!(app.orphan_worktrees_rx.is_none(), "no worker started");

        app.handle_key(key(KeyCode::Enter)).unwrap();
        app.handle_key(key(KeyCode::Char('b'))).unwrap();
        assert_eq!(
            stage(&app),
            OrphanWorktreesStage::Confirm {
                delete_branch: true
            }
        );

        app.handle_key(key(KeyCode::Enter)).unwrap();
        assert_eq!(stage(&app), OrphanWorktreesStage::Removing);
        assert!(app.orphan_worktrees_rx.is_some(), "the removal worker runs");
    }

    /// Fork test: the list shows the dirty warning, the branch and the path.
    #[test]
    fn orphan_cleaner_renders_dirty_and_untracked_warning() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let paths = app.engine.paths.clone();
        app.prompt = PromptState::OrphanWorktrees(OrphanWorktreesPrompt {
            candidates: vec![OrphanWorktreeCandidate {
                project_path: paths.root.clone(),
                worktree_path: paths.worktrees_root.join("dirty-orphan"),
                branch: Some("keep-this-branch".to_string()),
                dirty: true,
            }],
            selected: 0,
            stage: OrphanWorktreesStage::List,
        });

        let backend = TestBackend::new(200, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render orphan inventory");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("DIRTY"));
        assert!(rendered.contains("keep-this-branch"));
        assert!(rendered.contains("dirty-orphan"));
    }

    /// A detached orphan cannot opt into branch deletion.
    #[test]
    fn a_detached_row_cannot_toggle_branch_deletion() {
        let mut app = test_app(default_bindings());
        open_list(&mut app, vec![candidate("d", None, false)]);
        app.handle_orphan_worktrees_prompt_key(key(KeyCode::Enter));
        app.handle_orphan_worktrees_prompt_key(key(KeyCode::Char('b')));
        assert_eq!(
            stage(&app),
            OrphanWorktreesStage::Confirm {
                delete_branch: false
            }
        );
    }

    /// A successful removal drops exactly that row and says the branch was
    /// kept; the last one closes the cleaner. A failure keeps the row.
    #[test]
    fn removal_answers_update_the_list_honestly() {
        let mut app = test_app(default_bindings());
        open_list(
            &mut app,
            vec![
                candidate("a", Some("a"), false),
                candidate("b", Some("b"), false),
            ],
        );
        app.apply_orphan_worktrees_answer(OrphanWorktreesAnswer::Removed {
            worktree_path: PathBuf::from("/tmp/dux/worktrees/b"),
            delete_branch: false,
            result: Err("worktree is no longer an eligible orphan".to_string()),
        });
        let PromptState::OrphanWorktrees(prompt) = &app.prompt else {
            panic!("closed");
        };
        assert_eq!(prompt.candidates.len(), 2, "a failed removal keeps the row");

        app.apply_orphan_worktrees_answer(OrphanWorktreesAnswer::Removed {
            worktree_path: PathBuf::from("/tmp/dux/worktrees/a"),
            delete_branch: false,
            result: Ok(()),
        });
        let PromptState::OrphanWorktrees(prompt) = &app.prompt else {
            panic!("closed");
        };
        assert_eq!(prompt.candidates.len(), 1);
        assert_eq!(prompt.candidates[0].branch.as_deref(), Some("b"));
        assert!(app.status.snapshot()[0].message.contains("branch was kept"));

        app.apply_orphan_worktrees_answer(OrphanWorktreesAnswer::Removed {
            worktree_path: PathBuf::from("/tmp/dux/worktrees/b"),
            delete_branch: true,
            result: Ok(()),
        });
        assert!(matches!(app.prompt, PromptState::None));
    }

    /// A failed inventory closes the cleaner with the fail-closed reason and
    /// never shows a partial list.
    #[test]
    fn a_failed_inventory_closes_with_the_reason() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::OrphanWorktrees(OrphanWorktreesPrompt {
            candidates: Vec::new(),
            selected: 0,
            stage: OrphanWorktreesStage::Loading,
        });
        app.apply_orphan_worktrees_answer(OrphanWorktreesAnswer::Inventory(Err(
            "orphan cleanup aborted before removal: repair config".to_string(),
        )));
        assert!(matches!(app.prompt, PromptState::None));
        assert!(
            app.status.snapshot()[0]
                .message
                .contains("aborted before removal")
        );
    }

    /// End to end through the palette command and the real inventory worker:
    /// the test app's root is registered with no orphans, so the cleaner opens
    /// and then reports that nothing is eligible.
    #[test]
    fn the_palette_command_runs_the_inventory_off_thread() {
        let mut app = test_app(default_bindings());
        dux_core::storage::load_or_create_store_id(&app.engine.paths.root).unwrap();
        app.execute_command("prune-orphan-worktrees".to_string())
            .expect("command");
        assert!(matches!(app.prompt, PromptState::OrphanWorktrees(_)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while app.orphan_worktrees_rx.is_some() && std::time::Instant::now() < deadline {
            app.drain_orphan_worktrees();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.orphan_worktrees_rx.is_none(), "the worker answered");
        let text = app.status.snapshot()[0].message.clone();
        assert!(
            text.contains("No eligible orphan worktrees") || text.contains("aborted"),
            "{text}"
        );
    }
}
