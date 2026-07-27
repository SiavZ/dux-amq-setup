//! Worker events and blocking Git, filesystem, and provider jobs for the TUI.

use super::*;

const DETACHED_HEAD_LABEL: &str = "(detached HEAD)";

impl App {
    pub(crate) fn drain_events(&mut self) {
        while let Ok(event) = self.runtime.worker_rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentProgress(message) => self.set_busy(message),
                WorkerEvent::CreateAgentReady(boxed) => {
                    let AgentReadyData {
                        session,
                        client,
                        pty_size,
                        status_message,
                        fresh_capture,
                    } = *boxed;
                    self.create_agent_in_flight = false;
                    self.last_pty_size = pty_size;
                    if !session.shared_workspace() {
                        self.detach_conflicting_worktree_session(
                            &session.worktree_path,
                            &session.id,
                        );
                        self.ensure_project_worktree_link_for_project_id(&session.project_id);
                    }
                    let session_id = session.id.clone();
                    self.git.sessions.insert(0, session);
                    self.install_pty_for_session(
                        &session_id,
                        crate::pty::PtyHandle::new(client),
                    );
                    self.mark_session_provider_started(&session_id);
                    self.finish_fresh_capture(&session_id, fresh_capture);
                    self.update_branch_sync_sessions();
                    self.rebuild_left_items();
                    self.selected_left = self
                        .left_items()
                        .iter()
                        .position(|item| matches!(item, LeftItem::Session(index) if self.git.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(session_id.as_str())))
                        .unwrap_or(0);
                    self.reload_changed_files();
                    self.show_agent_surface();
                    self.ui.input_target = InputTarget::Agent;
                    self.ui.fullscreen_overlay = FullscreenOverlay::Agent;
                    if let Some(warning) = self.shared_targeted_resume_warning(&session_id) {
                        self.set_warning(warning);
                    } else {
                        self.set_info(status_message);
                    }
                }
                WorkerEvent::CreateAgentFailed(message) => {
                    self.create_agent_in_flight = false;
                    self.set_error(message);
                }
                WorkerEvent::CreateAgentRecoverable { session, message } => {
                    self.create_agent_in_flight = false;
                    let session = *session;
                    if !session.shared_workspace() {
                        self.ensure_project_worktree_link_for_project_id(&session.project_id);
                    }
                    let session_id = session.id.clone();
                    self.git.sessions.retain(|candidate| candidate.id != session_id);
                    self.git.sessions.insert(0, session);
                    self.update_branch_sync_sessions();
                    self.rebuild_left_items();
                    self.selected_left = self
                        .left_items()
                        .iter()
                        .position(|item| matches!(item, LeftItem::Session(index) if self.git.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(session_id.as_str())))
                        .unwrap_or(0);
                    self.reload_changed_files();
                    self.set_error(message);
                }
                WorkerEvent::ChangedFilesReady { staged, unstaged } => {
                    self.git.staged_files = staged;
                    self.git.unstaged_files = unstaged;
                    self.clamp_files_cursor();
                }
                WorkerEvent::CommitMessageGenerated(msg) => {
                    self.git.commit_input.clear_overlay();
                    self.git.commit_input.set_text(msg);
                    self.ui.input_target = InputTarget::CommitMessage;
                    {
                        let exit_key = self.bindings.label_for(Action::ExitCommitInput);
                        let commit_key = self.bindings.label_for(Action::CommitChanges);
                        self.set_info(format!(
                            "AI commit message generated. Press {exit_key} to exit, then {commit_key} to commit.",
                        ));
                    }
                }
                WorkerEvent::CommitMessageFailed(err) => {
                    self.git.commit_input.clear_overlay();
                    {
                        let gen_key = self.bindings.label_for(Action::GenerateCommitMessage);
                        self.set_error(format!(
                            "Failed to generate AI commit message: {err}. \
                             You can write one manually or retry with {gen_key}.",
                        ));
                    }
                }
                WorkerEvent::PushCompleted(result) => match result {
                    Ok(()) => self.set_info(
                        "Pushed to remote successfully. Your changes are now available to collaborators.",
                    ),
                    Err(e) => self.set_error(format!("Push to remote failed: {e}")),
                },
                WorkerEvent::PullCompleted {
                    repo_path,
                    target,
                    result,
                } => {
                    self.runtime.pulls_in_flight.remove(&repo_path);
                    match target {
                        PullTarget::Project {
                            project_id,
                            project_name,
                        } => match result {
                            Ok(branch_name) => {
                                if let Some(existing) = self.git
                                    .projects
                                    .iter_mut()
                                    .find(|candidate| candidate.id == project_id)
                                    && let Some(branch_name) = branch_name
                                {
                                    existing.current_branch = branch_name;
                                }
                                self.set_info(format!(
                                    "Refreshed project \"{project_name}\". Local branch is up to date with remote.",
                                ));
                            }
                            Err(e) => self
                                .set_error(format!("Project refresh failed for \"{project_name}\": {e}")),
                        },
                        PullTarget::Session => match result {
                            Ok(_) => {
                                self.set_info(
                                    "Pulled latest changes from remote successfully. Local branch is up to date.",
                                );
                                self.reload_changed_files();
                            }
                            Err(e) => self.set_error(format!("Pull from remote failed: {e}")),
                        },
                    }
                }
                WorkerEvent::ClipboardCopyCompleted { label, result } => match result {
                    Ok(()) => self.set_info(label),
                    Err(e) => self.set_error(format!("Clipboard copy failed: {e}")),
                },
                WorkerEvent::BranchRenameCompleted {
                    session_id,
                    worktree,
                    old_branch,
                    new_branch,
                    previous_title,
                    result,
                } => match result {
                    Ok(()) => {
                        let Some(index) = self
                            .git
                            .sessions
                            .iter()
                            .position(|session| session.id == session_id)
                        else {
                            dispatch_branch_rename_rollback(
                                self.runtime.worker_tx.clone(),
                                session_id,
                                worktree,
                                new_branch,
                                old_branch,
                            );
                            continue;
                        };
                        let mut candidate = self.git.sessions[index].clone();
                        candidate.branch_name = new_branch.clone();
                        candidate.updated_at = Utc::now();
                        if let Err(err) = self.session_store.upsert_session(&candidate) {
                            self.git.sessions[index].title = previous_title;
                            dispatch_branch_rename_rollback(
                                self.runtime.worker_tx.clone(),
                                session_id,
                                worktree,
                                new_branch,
                                old_branch,
                            );
                            self.rebuild_left_items();
                            self.set_error(format!(
                                "Branch was renamed but couldn't be persisted; restoring it: {err}"
                            ));
                            continue;
                        }
                        self.git.sessions[index].branch_name = candidate.branch_name;
                        self.git.sessions[index].updated_at = candidate.updated_at;
                        self.update_branch_sync_sessions();
                        self.rebuild_left_items();
                        self.set_info(format!(
                            "Renamed agent and branch to \"{new_branch}\"."
                        ));
                    }
                    Err(e) => {
                        // Revert the title so the session doesn't stay in a
                        // mixed state where the display name changed but the
                        // branch didn't.
                        if let Some(session) =
                            self.git.sessions.iter_mut().find(|s| s.id == session_id)
                        {
                            session.title = previous_title;
                            session.updated_at = Utc::now();
                        }
                        self.rebuild_left_items();
                        self.set_error(format!(
                            "Branch rename failed, reverted agent name: {e}"
                        ));
                    }
                },
                WorkerEvent::BranchRenameRollbackCompleted { session_id, result } => {
                    if let Err(err) = result {
                        self.set_error(format!(
                            "Couldn't restore branch after persistence failure for session {}: {err}",
                            crate::sanitize::for_terminal(&session_id)
                        ));
                    }
                }
                WorkerEvent::BranchSyncReady(updates) => {
                    self.apply_branch_sync_updates(updates);
                }
                WorkerEvent::GhStatusChecked(status) => {
                    self.runtime.gh_status = status;
                    if matches!(status, crate::model::GhStatus::Available)
                        && self.runtime.github_integration_enabled
                    {
                        logger::info("[gh-integration] gh CLI is available and authenticated");
                        self.update_pr_sync_sessions();
                        self.spawn_pr_sync_worker();
                        self.spawn_initial_pr_refresh();
                        self.spawn_refs_watcher();
                    } else {
                        logger::info(&format!(
                            "[gh-integration] gh status: {:?}, integration enabled: {}",
                            status, self.runtime.github_integration_enabled,
                        ));
                    }
                }
                WorkerEvent::PrStatusReady(results) => {
                    let now = Instant::now();
                    let mut changed = false;
                    for (session_id, maybe_pr) in results {
                        self.runtime.pr_last_checked.insert(session_id.clone(), now);
                        match maybe_pr {
                            Some(pr) => {
                                // Persist the PR association (including state) so it
                                // survives restarts and squash-merge branch deletions.
                                let state_str = match pr.state {
                                    crate::model::PrState::Open => "OPEN",
                                    crate::model::PrState::Merged => "MERGED",
                                    crate::model::PrState::Closed => "CLOSED",
                                };
                                let _ = self.session_store.upsert_pr(
                                    &session_id,
                                    pr.number,
                                    &pr.owner_repo,
                                    state_str,
                                    &pr.title,
                                );
                                self.runtime.pr_statuses.insert(session_id, pr);
                                changed = true;
                            }
                            None => {
                                if self.runtime.pr_statuses.remove(&session_id).is_some() {
                                    changed = true;
                                }
                            }
                        }
                    }
                    if changed {
                        // Refresh the sync entries so the worker has updated known_pr data.
                        self.update_pr_sync_sessions();
                        self.rebuild_left_items();
                    }
                }
                WorkerEvent::RefsChanged(session_id) => {
                    logger::debug(&format!(
                        "[gh-integration] refs watcher: triggering PR check for session {session_id}",
                    ));
                    self.spawn_pr_check_for_session(&session_id);
                }
                WorkerEvent::BrowserEntriesReady {
                    dir,
                    restore_dir,
                    result,
                } => {
                    let mut browser_error = None;
                    if let PromptState::BrowseProjects {
                        current_dir,
                        entries: current_entries,
                        loading,
                        selected,
                        ..
                    } = &mut self.ui.prompt
                        && *current_dir == dir
                    {
                        *loading = false;
                        *selected = 0;
                        match result {
                            Ok(entries) => *current_entries = entries,
                            Err(err) => {
                                current_entries.clear();
                                if let Some(previous) = restore_dir {
                                    *current_dir = previous;
                                }
                                browser_error = Some(err);
                            }
                        }
                    }
                    if let Some(err) = browser_error {
                        self.set_error(err);
                    }
                }
                WorkerEvent::WorktreeRemoveCompleted { session_id, result } => {
                    match result {
                        Ok(branch_already_deleted) => {
                            let needs_amq_worker = self
                                .git
                                .sessions
                                .iter()
                                .find(|session| session.id == session_id)
                                .is_some_and(|session| {
                                    crate::peer::amq_cleanup_requires_worker(
                                        &self.paths,
                                        &self.store_id,
                                        session,
                                    )
                                });
                            if needs_amq_worker {
                                self.dispatch_amq_session_delete(
                                    session_id,
                                    true,
                                    Some(branch_already_deleted),
                                );
                            } else {
                                self.git.pending_deletions.remove(&session_id);
                                let our_busy_msg = self
                                    .git
                                    .deletion_busy_messages
                                    .remove(&session_id);
                                let update_status = our_busy_msg.as_ref().is_some_and(|msg| {
                                    self.status.tone()
                                        == crate::statusline::StatusTone::Busy
                                        && self.status.message() == msg.as_str()
                                });
                                if self.git.sessions.iter().any(|s| s.id == session_id) {
                                    if let Err(e) = self.finish_delete_session(
                                        &session_id,
                                        true,
                                        Some(branch_already_deleted),
                                        update_status,
                                    ) {
                                        self.set_error(format!(
                                            "Worktree removed but session cleanup failed: {e:#}"
                                        ));
                                    }
                                } else if update_status {
                                    self.set_info("Worktree removal finished.");
                                }
                            }
                        }
                        Err(msg) => {
                            self.git.pending_deletions.remove(&session_id);
                            self.git.deletion_busy_messages.remove(&session_id);
                            // Session record is normally still present
                            // because we deferred cleanup until git
                            // succeeded. Look up the session label so the
                            // user knows which agent failed — multiple async
                            // deletes can be in flight concurrently, and a
                            // bare error would be ambiguous.
                            if let Some(session) =
                                self.git.sessions.iter().find(|s| s.id == session_id)
                            {
                                let name = session
                                    .title
                                    .as_deref()
                                    .unwrap_or(&session.branch_name);
                                self.set_error(format!(
                                    "Worktree delete failed for {} agent \"{name}\": {msg}",
                                    session.provider.as_str(),
                                ));
                            } else {
                                self.set_error(format!(
                                    "Worktree delete failed: {msg}"
                                ));
                            }
                        }
                    }
                }
                WorkerEvent::AmqDeleteCompleted {
                    session_id,
                    delete_worktree,
                    remove_outcome,
                    result,
                } => {
                    self.git.pending_deletions.remove(&session_id);
                    let our_busy_msg = self.git.deletion_busy_messages.remove(&session_id);
                    let update_status = our_busy_msg.as_ref().is_some_and(|msg| {
                        self.status.tone() == crate::statusline::StatusTone::Busy
                            && self.status.message() == msg.as_str()
                    });
                    match result {
                        Ok(()) => {
                            if let Err(err) = self.finish_delete_session_after_amq(
                                &session_id,
                                delete_worktree,
                                remove_outcome,
                                update_status,
                            ) {
                                self.set_error(format!("Session cleanup failed: {err:#}"));
                            }
                        }
                        Err(err) => self.set_error(format!(
                            "Message-delivery cleanup failed; session retained for retry: {err}"
                        )),
                    }
                }
                WorkerEvent::ResourceStatsReady(stats) => {
                    self.resource_stats_in_flight = false;
                    if let PromptState::ResourceMonitor {
                        rows,
                        selected_row,
                        expanded,
                        last_refresh,
                        first_sample,
                        ..
                    } = &mut self.ui.prompt
                    {
                        *rows = stats;
                        *last_refresh = Instant::now();
                        *first_sample = false;
                        // Clamp cursor to the (possibly changed) visual row count.
                        let visual = build_visual_rows(rows, expanded);
                        let max_row = visual.len().saturating_sub(1);
                        if *selected_row > max_row {
                            *selected_row = max_row;
                        }
                    }
                }
                WorkerEvent::AddProjectCheckoutCompleted {
                    path,
                    name,
                    target_branch,
                    result,
                } => match result {
                    Ok(()) => {
                        let display_name = if name.trim().is_empty() {
                            std::path::Path::new(&path)
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("project")
                                .to_string()
                        } else {
                            name.trim().to_string()
                        };
                        if let Err(e) =
                            self.finish_add_project(path, name, target_branch.clone())
                        {
                            self.set_error(format!("{e:#}"));
                        } else {
                            // Override the generic "Added project" status from
                            // finish_add_project with the more informative
                            // two-step message.
                            self.set_info(format!(
                                "Checked out \"{target_branch}\" and added project \"{display_name}\" to workspace."
                            ));
                        }
                    }
                    Err(err) => {
                        // Preserve the full git stderr in the log so
                        // debugging stays possible after the status line
                        // summary is overwritten by the next message.
                        logger::error(&format!(
                            "add-project checkout failed for {path}: {err}"
                        ));
                        self.set_error(format!(
                            "Couldn't check out \"{target_branch}\" in {path} — resolve in your terminal and retry."
                        ));
                    }
                },
                WorkerEvent::ProjectMetaReady {
                    path,
                    is_git,
                    current_branch,
                    head_detached,
                    remote_default: _,
                } => {
                    let path_str = path.to_string_lossy().to_string();
                    if let Some(index) = self
                        .git
                        .projects
                        .iter()
                        .position(|project| Path::new(&project.path) == path.as_path())
                    {
                        let project_id = self.git.projects[index].id.clone();
                        let shared_project = self
                            .config
                            .workspace_mode_for_project_id(&project_id)
                            == WorkspaceMode::Shared
                            || self.git.sessions.iter().any(|session| {
                                session.project_id == project_id && session.shared_workspace()
                            });
                        let head_resolved = head_detached || current_branch.is_some();
                        let branch = if is_git && shared_project && head_detached {
                            DETACHED_HEAD_LABEL.to_string()
                        } else if is_git {
                            current_branch.unwrap_or_else(|| "main".to_string())
                        } else {
                            String::new()
                        };
                        let proj = &mut self.git.projects[index];
                        proj.path_missing = !is_git;
                        proj.current_branch = branch.clone();
                        proj.meta_loaded = true;
                        if is_git
                            && head_resolved
                            && shared_project
                        {
                            let updates = self
                                .git
                                .sessions
                                .iter()
                                .filter(|session| {
                                    session.project_id == project_id
                                        && session.shared_workspace()
                                        && session.branch_name != branch
                                })
                                .map(|session| (session.id.clone(), branch.clone()))
                                .collect();
                            self.apply_branch_sync_updates(updates);
                        }
                    } else {
                        logger::info(&format!(
                            "ProjectMetaReady arrived for unknown project path {path_str}; \
                             discarding (project may have been removed)"
                        ));
                    }
                }
                WorkerEvent::ReloadChangedFilesReady { worktree, result } => {
                    // Discard out-of-order replies: by the time the worker
                    // returned, the user may have switched to a different
                    // session. The currently-selected worktree wins.
                    let current = self
                        .selected_session()
                        .map(|s| PathBuf::from(&s.worktree_path));
                    if current.as_deref() != Some(worktree.as_path()) {
                        continue;
                    }
                    match result {
                        Ok((staged, unstaged)) => {
                            self.git.staged_files = staged;
                            self.git.unstaged_files = unstaged;
                            self.clamp_files_cursor();
                        }
                        Err(err) => {
                            logger::error(&format!(
                                "changed_files refresh failed for {}: {err}",
                                worktree.display()
                            ));
                            // Leave the lists empty — the steady-state
                            // poller will retry on its next tick.
                        }
                    }
                }
                WorkerEvent::StagedDiffReady { worktree, result } => {
                    self.git.staged_diff_in_flight = false;
                    match result {
                        Ok(diff) => {
                            self.launch_commit_message_provider(worktree, diff);
                        }
                        Err(err) => {
                            self.git.commit_input.clear_overlay();
                            self.set_error(format!("Failed to read staged diff: {err}"));
                        }
                    }
                }
                WorkerEvent::GitFileOperationCompleted {
                    action,
                    worktree,
                    path,
                    result,
                } => match result {
                    Ok(()) => {
                        if let GitFileAction::Discard { .. } = action {
                            self.set_info(format!(
                                "Discarded changes to \"{path}\". File restored to last committed state."
                            ));
                        }
                        if self
                            .selected_session()
                            .is_some_and(|session| Path::new(&session.worktree_path) == worktree)
                        {
                            // Mirror the pre-worker synchronous behavior: a
                            // stage/unstage that empties its source section
                            // moves focus to the other section so the pane
                            // isn't left on an empty list. The current lists
                            // still reflect pre-operation state here.
                            if let Some(next) = section_after_git_file_op(
                                &action,
                                self.git.staged_files.len(),
                                self.git.unstaged_files.len(),
                            ) {
                                self.right_section = next;
                            }
                            self.reload_changed_files();
                        }
                    }
                    Err(err) => {
                        let operation = match action {
                            GitFileAction::Stage => "Stage",
                            GitFileAction::Unstage => "Unstage",
                            GitFileAction::Discard { .. } => "Discard",
                        };
                        self.set_error(format!("{operation} failed for \"{path}\": {err}"));
                    }
                },
                WorkerEvent::PathCompletionsReady {
                    query,
                    reverse,
                    candidates,
                } => {
                    if let PromptState::BrowseProjects {
                        editing_path: true,
                        path_input,
                        tab_completions,
                        tab_index,
                        ..
                    } = &mut self.ui.prompt
                        && path_input.text == query
                    {
                        *tab_completions = candidates;
                        if !tab_completions.is_empty() {
                            *tab_index = if reverse {
                                tab_completions.len() - 1
                            } else {
                                0
                            };
                            path_input.set_text(tab_completions[*tab_index].clone());
                        }
                    }
                }
                WorkerEvent::DiffReady {
                    worktree,
                    rel_path,
                    scroll,
                    focus_when_ready,
                    result,
                } => {
                    let still_relevant = if focus_when_ready {
                        self.selected_session()
                            .is_some_and(|session| session.worktree_path == worktree)
                    } else {
                        matches!(
                            &self.center_mode,
                            CenterMode::Diff {
                                worktree_path,
                                rel_path: current_path,
                                ..
                            } if worktree_path == &worktree && current_path == &rel_path
                        )
                    };
                    if !still_relevant {
                        continue;
                    }
                    match result {
                        Ok(output) => {
                            self.center_mode = CenterMode::Diff {
                                lines: Arc::new(output.lines),
                                scroll,
                                gutter_width: output.gutter_width,
                                worktree_path: worktree,
                                rel_path,
                            };
                            if focus_when_ready {
                                self.ui.focus = FocusPane::Center;
                            }
                        }
                        Err(err) => self.set_error(format!("Couldn't render diff: {err}")),
                    }
                }
                WorkerEvent::ConfigSaveCompleted {
                    success,
                    rollback,
                    result,
                } => match result {
                    Ok(()) => {
                        if !success.is_empty() {
                            self.set_info(success);
                        }
                    }
                    Err(err) => {
                        match rollback {
                            ConfigSaveRollback::None => {}
                            ConfigSaveRollback::DefaultProvider(previous) => {
                                self.config.defaults.provider = previous;
                                refresh_project_defaults(&mut self.git.projects, &self.config);
                                self.rebuild_left_items();
                            }
                            ConfigSaveRollback::Theme(previous) => {
                                self.config.ui.theme = previous;
                            }
                        }
                        let context = if success.is_empty() {
                            "config change"
                        } else {
                            success.trim_end_matches('.')
                        };
                        self.set_error(format!("Couldn't persist {context} to config: {err}"));
                    }
                },
                WorkerEvent::CommitFinished {
                    worktree: _,
                    message: _,
                    result,
                } => {
                    self.git.commit_in_flight = false;
                    self.git.commit_input.clear_overlay();
                    match result {
                        Ok(()) => {
                            self.git.commit_input.clear();
                            let push_key = self.bindings.label_for(Action::PushToRemote);
                            let ai_key = self.bindings.label_for(Action::GenerateCommitMessage);
                            self.set_info(format!(
                                "Changes committed successfully. Press {push_key} to push to remote, or {ai_key} to generate an AI message."
                            ));
                            self.reload_changed_files();
                        }
                        Err(err) => self.set_error(format!("Commit failed: {err}")),
                    }
                }
                WorkerEvent::AutoResumeSpawnOnMain {
                    session,
                    launch,
                    fresh_capture,
                    ack,
                } => {
                    // The fork MUST happen here on the main thread (macOS
                    // fork-safety) — see the WorkerEvent variant docs.
                    match fresh_capture {
                        Ok(capture) => self.spawn_auto_resume_on_main(*session, launch, capture),
                        Err(err) => {
                            self.git
                                .fresh_launches_in_flight
                                .remove(&session.id);
                            tracing::warn!(
                                target: "dux::resume_recovery",
                                session_id = %crate::sanitize::for_terminal(&session.id),
                                provider = %crate::sanitize::for_terminal(session.provider.as_str()),
                                error = %crate::sanitize::for_terminal(&err),
                                "auto-resume fresh-session capture preparation failed",
                            );
                            self.set_warning(format!(
                                "Could not auto-start agent \"{}\": exact {} conversation capture could not be prepared: {err}",
                                self.session_label(&session),
                                session.provider.as_str(),
                            ));
                        }
                    }
                    // Release the scheduler's throttle slot regardless of
                    // outcome; it only bounds provider boots in flight.
                    let _ = ack.send(());
                }
                WorkerEvent::FreshLaunchPrepared {
                    session,
                    context,
                    result,
                } => self.handle_prepared_fresh_launch(*session, context, result),
                WorkerEvent::ProviderSessionCaptured {
                    session_id,
                    provider,
                    result,
                } => self.handle_provider_session_captured(&session_id, &provider, result),
                WorkerEvent::ResumeRecoveryCompleted(result) => {
                    self.handle_resume_recovery_completed(result);
                }
                WorkerEvent::DiskUsage(pct) => {
                    self.handle_disk_usage_event(pct);
                }
                WorkerEvent::ScrollbackUsage(scrollback_lines) => {
                    self.handle_scrollback_usage_event(scrollback_lines);
                }
                WorkerEvent::AmqInjectScanRequested => {
                    self.drain_inject_queue_dir();
                }
                WorkerEvent::AddProjectMetaReady {
                    path,
                    name,
                    workspace_mode,
                    result,
                } => {
                    self.git.add_project_in_flight = false;
                    match result {
                        Ok(meta) => {
                            if let Err(e) = self.resume_add_project_after_meta(
                                path,
                                name,
                                workspace_mode,
                                meta,
                            ) {
                                self.set_error(format!("{e:#}"));
                            }
                        }
                        Err(err) => {
                            logger::error(&format!(
                                "add project rejected for {}: {err}",
                                path.display()
                            ));
                            self.set_error(err);
                        }
                    }
                }
                WorkerEvent::SharedReconnectValidated {
                    session_id,
                    force_fresh,
                    result,
                } => {
                    self.git
                        .reconnect_validations_in_flight
                        .remove(&session_id);
                    if let Err(message) = result {
                        self.set_error(message);
                        continue;
                    }
                    let reconnect = self
                        .continue_shared_reconnect_after_validation(&session_id, force_fresh);
                    if let Err(err) = reconnect {
                        self.set_error(crate::sanitize::for_terminal(&format!("{err:#}")));
                    }
                }
                WorkerEvent::OrphanWorktreesReady(result) => {
                    self.git.orphan_cleanup_in_flight = false;
                    match result {
                        Ok(candidates) if candidates.is_empty() => {
                            self.ui.prompt = PromptState::None;
                            self.set_info("No eligible orphan worktrees found.");
                        }
                        Ok(candidates) => {
                            self.ui.prompt = PromptState::OrphanWorktrees {
                                candidates,
                                selected: 0,
                            };
                            self.set_info(
                                "Orphan worktree inventory loaded; review each item before removal.",
                            );
                        }
                        Err(message) => {
                            self.ui.prompt = PromptState::None;
                            self.set_error(crate::sanitize::for_terminal(&message));
                        }
                    }
                }
                WorkerEvent::OrphanWorktreeRemoved {
                    candidate,
                    mut candidates,
                    selected,
                    delete_branch,
                    result,
                } => {
                    self.git.orphan_cleanup_in_flight = false;
                    match result {
                        Ok(()) => {
                            candidates.retain(|item| {
                                item.worktree_path != candidate.worktree_path
                            });
                            let safe_path = crate::sanitize::for_terminal(
                                &candidate.worktree_path.display().to_string(),
                            );
                            if candidates.is_empty() {
                                self.ui.prompt = PromptState::None;
                            } else {
                                let selected = selected.min(candidates.len() - 1);
                                self.ui.prompt = PromptState::OrphanWorktrees {
                                    candidates,
                                    selected,
                                };
                            }
                            let branch_note = if delete_branch {
                                " and requested branch deletion"
                            } else {
                                "; branch preserved"
                            };
                            self.set_info(format!(
                                "Removed orphan worktree {safe_path}{branch_note}."
                            ));
                        }
                        Err(message) => {
                            self.ui.prompt = PromptState::OrphanWorktrees {
                                candidates,
                                selected,
                            };
                            self.set_error(crate::sanitize::for_terminal(&message));
                        }
                    }
                }
            }
        }
        self.retry_hung_resume_sessions();
        // Detect PTY exits by walking sessions whose state owns a PTY.
        let mut exited = Vec::new();
        for session in self.git.sessions.iter_mut() {
            let Some(handle) = session.state.pty_handle_mut() else {
                continue;
            };
            if handle.is_exited() || handle.try_wait().is_some() {
                exited.push(session.id.clone());
            }
        }

        // For sessions that were spawned with resume_args and exited before
        // producing any output, retry with regular args (fresh session).
        // This handles `claude --continue || claude` style fallback.
        let mut retried = HashSet::new();
        for session_id in &exited {
            if self
                .git
                .resume_fallback_candidates
                .remove(session_id)
                .is_none()
            {
                continue;
            }
            // Check whether the exited process produced only minimal output
            // (no scrollback and ≤5 visible lines). A failed `--continue`
            // typically prints 1-2 lines of error; a real session produces
            // far more output and scrollback history.
            let is_minimal = self
                .find_pty_handle(session_id)
                .map(|p| p.has_minimal_output(5))
                .unwrap_or(true);
            if !is_minimal {
                continue;
            }
            let Some(session) = self
                .git
                .sessions
                .iter()
                .find(|s| s.id == *session_id)
                .cloned()
            else {
                continue;
            };
            // Drop the previous PTY (kills child + joins reader).
            let _ = self.take_session_pty(session_id);
            self.runtime.running_provider_pins.remove(session_id);
            self.git.last_pty_activity.remove(session_id);
            logger::info(&format!(
                "resume args exited without output for agent \"{}\", retrying with regular args",
                session.branch_name
            ));
            let proj_name = self.project_name_for_session(&session);
            self.launch_fresh_with_capture(
                session.clone(),
                FreshLaunchContext {
                    success_message: format!(
                        "No prior session to resume for agent \"{}\". Started a fresh {} session in project \"{}\".",
                        session.branch_name,
                        session.provider.as_str(),
                        proj_name,
                    ),
                    failure_prefix: format!(
                        "Fallback PTY spawn failed for agent \"{}\"",
                        session.branch_name
                    ),
                    show_agent_surface: false,
                },
            );
            retried.insert(session_id.clone());
        }

        for session_id in &exited {
            if retried.contains(session_id) {
                continue;
            }
            self.runtime.running_provider_pins.remove(session_id);
            self.git.last_pty_activity.remove(session_id);
            // Child has exited — `mark_session_exited` drops the
            // (possibly already-dead) PTY handle.
            self.mark_session_exited(session_id, None);
        }
        if !exited.is_empty() {
            // If the currently-viewed session just exited (and was not retried),
            // leave interactive mode.
            if let Some(current) = self.selected_session()
                && exited.contains(&current.id)
                && !retried.contains(&current.id)
            {
                let key = self.bindings.label_for(Action::ReconnectAgent);
                if self.session_surface == SessionSurface::Agent {
                    self.ui.input_target = InputTarget::None;
                    self.ui.fullscreen_overlay = FullscreenOverlay::None;
                    self.ui.focus = FocusPane::Left;
                    self.set_info(format!(
                        "Agent CLI process has exited. Press \"{key}\" to relaunch."
                    ));
                } else {
                    self.set_info(format!(
                        "Agent CLI process exited. Companion terminal is still available; press \"{key}\" to relaunch the agent."
                    ));
                }
            }
            // Trigger PR status check for exited agents.
            for sid in &exited {
                if !retried.contains(sid) {
                    self.spawn_pr_check_for_session(sid);
                }
            }
        }

        let mut exited_terminal_ids = Vec::new();
        for (terminal_id, terminal) in &mut self.runtime.companion_terminals {
            if terminal.client.is_exited() || terminal.client.try_wait().is_some() {
                exited_terminal_ids.push(terminal_id.clone());
            }
        }
        for terminal_id in &exited_terminal_ids {
            self.runtime.companion_terminals.remove(terminal_id);
        }
        if !exited_terminal_ids.is_empty() {
            // If the active terminal just exited, close the overlay.
            if let Some(ref active_id) = self.active_terminal_id
                && exited_terminal_ids.contains(active_id)
            {
                self.active_terminal_id = None;
                if self.ui.input_target == InputTarget::Terminal {
                    self.ui.input_target = InputTarget::None;
                }
                self.ui.fullscreen_overlay = FullscreenOverlay::None;
                self.session_surface = SessionSurface::Agent;
                self.set_info("Terminal exited. Press the terminal key to launch a new one.");
            }
            self.clamp_terminal_cursor();
        }

        // Poll foreground process names every ~2 seconds (every 20 ticks).
        if self.tick_count.is_multiple_of(20) {
            for terminal in self.runtime.companion_terminals.values_mut() {
                terminal.foreground_cmd = terminal.client.foreground_process_name();
            }
        }

        // Spawn a background worker to refresh resource monitor stats when
        // the overlay is open and enough wall-clock time has elapsed (~2s).
        if let PromptState::ResourceMonitor {
            ref last_refresh, ..
        } = self.ui.prompt
            && last_refresh.elapsed() >= Duration::from_secs(2)
        {
            self.spawn_resource_stats_worker();
        }

        // Keep the poller's interval flag in sync with whether any runtime PTY is alive.
        self.runtime
            .has_active_processes
            .store(self.running_process_count() > 0, Ordering::Relaxed);
    }

    pub(crate) fn spawn_browser_entries(&self, dir: &Path) {
        self.spawn_browser_entries_with_restore(dir, None);
    }

    pub(crate) fn spawn_browser_path_validation(&self, dir: &Path, previous: PathBuf) {
        self.spawn_browser_entries_with_restore(dir, Some(previous));
    }

    fn spawn_browser_entries_with_restore(&self, dir: &Path, restore_dir: Option<PathBuf>) {
        let tx = self.runtime.worker_tx.clone();
        let dir = dir.to_path_buf();
        thread::spawn(move || {
            let result = browser_entries(&dir);
            if let Ok(entries) = &result {
                logger::debug(&format!(
                    "browser loaded {} with {} entries",
                    dir.display(),
                    entries.len()
                ));
            }
            let _ = tx.send(WorkerEvent::BrowserEntriesReady {
                dir: dir.clone(),
                restore_dir,
                result,
            });
        });
    }

    /// Handle a [`WorkerEvent::DiskUsage`] sample.
    ///
    /// Caches the percentage on `self.disk_usage_pct` so
    /// [`super::sessions::App::refuse_agent_spawn_for_limits`] can refuse
    /// new agents at boot AND between samples, then decides whether to
    /// flash a status-line banner:
    ///
    /// * `pct >= disk_high_water_pct` → red error banner; new spawns are
    ///   refused by the gate above.
    /// * `pct >= disk_warn_pct` → yellow warning banner; spawns still
    ///   allowed.
    /// * otherwise → if the previous tone was Warning/Error and the
    ///   message looks like a disk banner we set, clear it back to
    ///   Info.
    pub(crate) fn handle_disk_usage_event(&mut self, pct: u8) {
        let high = self.config.limits.disk_high_water_pct;
        let warn = self.config.limits.disk_warn_pct;
        let prev = self.disk_usage_pct;
        self.disk_usage_pct = Some(pct);
        if pct >= high {
            self.set_error(format!(
                "Persistent disk at {pct}% (limits.disk_high_water_pct = {high}%); \
                 new agents refused. Run `dux session purge` or extend the volume."
            ));
        } else if pct >= warn {
            self.set_warning(format!(
                "Persistent disk at {pct}% (limits.disk_warn_pct = {warn}%); \
                 consider running `dux session purge`."
            ));
        } else if let Some(prev_pct) = prev
            && prev_pct >= warn
            && matches!(
                self.status.tone(),
                crate::statusline::StatusTone::Warning | crate::statusline::StatusTone::Error
            )
            && self.status.message().contains("Persistent disk at")
        {
            // The previous tick had us in warn/high-water territory and
            // we wrote the banner; now we're back below warn. Only clear
            // if the status line still shows our banner — otherwise
            // another subsystem owns it and we mustn't clobber.
            self.set_info("");
        }
    }

    /// Handle a [`WorkerEvent::ScrollbackUsage`] tick.
    ///
    /// This event is only emitted when
    /// `[limits].enable_scrollback_overflow_autodetach = true`. Computes
    /// the current total scrollback footprint (summed across all live
    /// PTYs) and, if it exceeds `[limits].max_total_scrollback_mb`,
    /// detaches the oldest-by-`updated_at` panes one at a time until the
    /// total drops back under the cap. Detach != kill: the session row
    /// stays in sqlite and can be reattached manually.
    pub(crate) fn handle_scrollback_usage_event(&mut self, scrollback_lines: usize) {
        let cap_bytes = self.config.limits.max_total_scrollback_mb.saturating_mul(
            // 1 MiB
            1024 * 1024,
        );
        if cap_bytes == 0 {
            return; // cap disabled
        }
        loop {
            let footprint = self.estimate_total_scrollback_bytes(scrollback_lines);
            if footprint <= cap_bytes {
                break;
            }
            // Pick the oldest-by-updated_at active pane and detach it.
            let Some(victim) = self.oldest_active_session_id() else {
                break; // nothing left to detach
            };
            // Drop the PTY handle on the victim — `take_session_pty`
            // returns the handle, and letting it fall out of scope
            // here kills the child + joins the reader thread, freeing
            // the grid memory and PTY-backed resources.
            let Some(_handle) = self.take_session_pty(&victim) else {
                break;
            };
            self.runtime.running_provider_pins.remove(&victim);
            self.git.last_pty_activity.remove(&victim);
            self.git.resume_fallback_candidates.remove(&victim);
            // Child has exited — `mark_session_exited` drops the
            // (possibly already-dead) PTY handle.
            self.mark_session_exited(&victim, None);
            logger::info(&format!(
                "scrollback watchdog auto-detached session {victim}: total \
                 grid footprint exceeded {} MiB",
                self.config.limits.max_total_scrollback_mb,
            ));
            self.set_warning(format!(
                "Auto-detached oldest agent: total scrollback exceeded \
                 limits.max_total_scrollback_mb = {} MiB.",
                self.config.limits.max_total_scrollback_mb,
            ));
        }
    }

    /// Approximates total scrollback grid memory across every live PTY.
    /// Each cell is roughly `4 bytes` (alacritty_terminal stores a
    /// glyph + style). We don't ask the terminal for an exact byte
    /// count because the watchdog only needs to know whether we're over
    /// the cap, not the precise number — and a per-cell walk per
    /// minute would be wasted work.
    pub(crate) fn estimate_total_scrollback_bytes(&self, scrollback_lines: usize) -> usize {
        // Default cols when the PTY hasn't been resized yet.
        let cols = if self.last_pty_size.1 == 0 {
            80usize
        } else {
            self.last_pty_size.1 as usize
        };
        const BYTES_PER_CELL: usize = 4;
        self.live_pty_count()
            .saturating_mul(scrollback_lines)
            .saturating_mul(cols)
            .saturating_mul(BYTES_PER_CELL)
    }

    /// Returns the ID of the oldest-by-`updated_at` session whose PTY is
    /// still attached. Used by the scrollback watchdog to pick a detach
    /// victim. Sessions without a live `PtyClient` are ignored.
    pub(crate) fn oldest_active_session_id(&self) -> Option<String> {
        self.git
            .sessions
            .iter()
            .filter(|s| s.state.has_pty())
            .min_by_key(|s| s.updated_at)
            .map(|s| s.id.clone())
    }

    /// Sample persistent-disk usage at `paths.root` once a minute and ship
    /// the percentage back to the UI thread via
    /// [`WorkerEvent::DiskUsage`].
    ///
    /// Uses [`rustix::fs::statvfs`] (already a transitive dependency).
    /// `statvfs` on a bind-mount reports the underlying filesystem's
    /// stats, which is the correct behaviour for a watchdog that's trying
    /// to refuse new agents before the host fills up — a bind-mounted dux
    /// home that points at `/data` should refuse new agents when `/data`
    /// is full, not when the bind point alone is full.
    pub(crate) fn spawn_disk_watchdog(&self) {
        let tx = self.runtime.worker_tx.clone();
        let root = self.paths.root.clone();
        let shutdown = Arc::clone(&self.runtime.shutdown);
        thread::Builder::new()
            .name("disk-watchdog".into())
            .spawn(move || {
                // Emit a first sample immediately so the UI can refuse
                // spawns at boot if the disk is already over high-water.
                if let Some(pct) = sample_disk_usage_pct(&root)
                    && tx.send(WorkerEvent::DiskUsage(pct)).is_err()
                {
                    return;
                }
                while !shutdown.load(Ordering::Relaxed) {
                    thread::sleep(DISK_WATCHDOG_INTERVAL);
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(pct) = sample_disk_usage_pct(&root) else {
                        continue;
                    };
                    if tx.send(WorkerEvent::DiskUsage(pct)).is_err() {
                        return; // receiver dropped, app shutting down
                    }
                }
            })
            .ok();
    }

    /// Sample total scrollback grid memory across all live PTYs once a
    /// minute and ship the result back via
    /// [`WorkerEvent::ScrollbackUsage`].
    ///
    /// The handler in `drain_events` decides whether to act on the value;
    /// auto-detach is gated on
    /// `[limits].enable_scrollback_overflow_autodetach` so this watchdog
    /// is started only when that flag is `true`.
    pub(crate) fn spawn_scrollback_watchdog(&self) {
        let tx = self.runtime.worker_tx.clone();
        let scrollback_lines = self.config.ui.agent_scrollback_lines;
        let shutdown = Arc::clone(&self.runtime.shutdown);
        // We can't safely peek at the live `PtyClient`s from a worker
        // thread (they hold non-Send fds), so the worker fires a tick on
        // a fixed interval and the UI thread computes the footprint when
        // it drains the event. The footprint is therefore an O(n_panes)
        // computation on the UI thread, but n is bounded by `max_panes`
        // (default 16) and the math is just per-pane multiplication, so
        // it's well under a millisecond.
        thread::Builder::new()
            .name("scrollback-watchdog".into())
            .spawn(move || {
                while !shutdown.load(Ordering::Relaxed) {
                    thread::sleep(SCROLLBACK_WATCHDOG_INTERVAL);
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    // We don't have access to the runtime pane list here;
                    // signal "tick" by sending a sentinel and let the UI
                    // thread compute the actual footprint with the
                    // configured scrollback line count.
                    if tx
                        .send(WorkerEvent::ScrollbackUsage(scrollback_lines))
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .ok();
    }

    pub(crate) fn spawn_branch_sync_worker(&self) {
        let interval_secs = self.config.ui.branch_sync_interval;
        let shared_only = interval_secs == 0;
        if shared_only {
            let shared_configured = self.config.default_workspace_mode() == WorkspaceMode::Shared
                || self.config.projects.iter().any(|project| {
                    self.config.workspace_mode_for_project(project) == WorkspaceMode::Shared
                })
                || self.git.sessions.iter().any(AgentSession::shared_workspace);
            if !shared_configured {
                return;
            }
        }
        let tx = self.runtime.worker_tx.clone();
        let sessions = Arc::clone(&self.runtime.branch_sync_sessions);
        let shutdown = Arc::clone(&self.runtime.shutdown);
        thread::spawn(move || {
            let interval = Duration::from_secs(if shared_only {
                45
            } else {
                u64::from(interval_secs)
            });
            while !shutdown.load(Ordering::Relaxed) {
                thread::sleep(interval);
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let mut snapshot = match sessions.lock() {
                    Ok(guard) => guard.clone(),
                    Err(_) => continue,
                };
                if shared_only {
                    snapshot.retain(|entry| entry.shared_workspace);
                }
                let updates = Self::collect_branch_sync_updates_with(&snapshot, git::head_branch);
                if !updates.is_empty() && tx.send(WorkerEvent::BranchSyncReady(updates)).is_err() {
                    break; // receiver dropped, app is shutting down
                }
            }
        });
    }

    fn collect_branch_sync_updates_with<F>(
        snapshot: &[BranchSyncEntry],
        mut head_branch: F,
    ) -> Vec<(String, String)>
    where
        F: FnMut(&Path) -> Result<Option<String>>,
    {
        let mut updates = Vec::new();
        let mut shared_groups: Vec<(PathBuf, Vec<&BranchSyncEntry>)> = Vec::new();
        for entry in snapshot {
            if !entry.shared_workspace {
                if let Ok(Some(actual)) = head_branch(Path::new(&entry.worktree_path))
                    && actual != entry.branch_name
                {
                    updates.push((entry.session_id.clone(), actual));
                }
                continue;
            }
            let path = PathBuf::from(&entry.worktree_path);
            let canonical = path.canonicalize().unwrap_or(path);
            if let Some((_, entries)) = shared_groups
                .iter_mut()
                .find(|(candidate, _)| *candidate == canonical)
            {
                entries.push(entry);
            } else {
                shared_groups.push((canonical, vec![entry]));
            }
        }
        for (path, entries) in shared_groups {
            let Ok(branch) = head_branch(&path) else {
                continue;
            };
            let actual = branch.unwrap_or_else(|| DETACHED_HEAD_LABEL.to_string());
            for entry in entries {
                if entry.branch_name != actual {
                    updates.push((entry.session_id.clone(), actual.clone()));
                }
            }
        }
        updates
    }

    // -- Git refs watcher for push detection --

    pub(crate) fn spawn_refs_watcher(&mut self) {
        use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};

        let tx = self.runtime.worker_tx.clone();
        // Build a reverse map of watched paths for event routing.
        let path_to_session: Arc<Mutex<HashMap<PathBuf, String>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let path_map = Arc::clone(&path_to_session);
        let debounce_map: Arc<Mutex<HashMap<String, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let debounce = Arc::clone(&debounce_map);

        let watcher_result = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else { return };
                // We only care about data modifications (ref file updates).
                if !event.kind.is_modify() && !event.kind.is_create() {
                    return;
                }
                let map = match path_map.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let mut debounce_guard = match debounce.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                for event_path in &event.paths {
                    // Walk up from the event path to find a watched parent dir.
                    for (watched, session_id) in map.iter() {
                        if event_path.starts_with(watched) {
                            // Debounce: skip if we already sent an event within the last 5s.
                            let now = Instant::now();
                            if let Some(last) = debounce_guard.get(session_id)
                                && now.duration_since(*last) < Duration::from_secs(5)
                            {
                                continue;
                            }
                            debounce_guard.insert(session_id.clone(), now);
                            logger::debug(&format!(
                                "[gh-integration] refs watcher: detected change at {}, debouncing for session {}",
                                event_path.display(),
                                session_id,
                            ));
                            let _ = tx.send(WorkerEvent::RefsChanged(session_id.clone()));
                        }
                    }
                }
            },
            NotifyConfig::default(),
        );

        match watcher_result {
            Ok(watcher) => {
                self.runtime.refs_watcher = Some(Arc::new(Mutex::new(watcher)));
                self.runtime.refs_watch_paths.clear();
                // Populate the path map and start watching existing sessions.
                let mut paths = HashMap::new();
                for session in &self.git.sessions {
                    let refs_dir = PathBuf::from(&session.worktree_path)
                        .join(".git")
                        .join("refs")
                        .join("heads");
                    if refs_dir.is_dir()
                        && let Some(ref watcher_arc) = self.runtime.refs_watcher
                        && let Ok(mut w) = watcher_arc.lock()
                    {
                        match w.watch(&refs_dir, RecursiveMode::NonRecursive) {
                            Ok(()) => {
                                logger::debug(&format!(
                                    "[gh-integration] refs watcher: watching {} for session {}",
                                    refs_dir.display(),
                                    session.id,
                                ));
                                paths.insert(refs_dir.clone(), session.id.clone());
                            }
                            Err(e) => {
                                logger::debug(&format!(
                                    "[gh-integration] refs watcher: failed to watch {}: {}",
                                    refs_dir.display(),
                                    e,
                                ));
                            }
                        }
                    }
                }
                self.runtime.refs_watch_paths = paths.clone();
                // Populate the closure's path map so events can route to sessions.
                if let Ok(mut map) = path_to_session.lock() {
                    *map = paths;
                }
                logger::info(&format!(
                    "[gh-integration] refs watcher: initialized, watching {} session(s)",
                    self.runtime.refs_watch_paths.len(),
                ));
            }
            Err(e) => {
                logger::warn(&format!(
                    "[gh-integration] refs watcher: failed to create watcher (falling back to poll-only): {e}",
                ));
            }
        }
    }

    // -- GitHub PR integration workers --

    pub(crate) fn spawn_gh_status_check(&self) {
        if !self.runtime.github_integration_enabled {
            return;
        }
        let tx = self.runtime.worker_tx.clone();
        thread::spawn(move || {
            use crate::model::GhStatus;
            // Step 1: Is `gh` on PATH?
            let on_path = std::process::Command::new("which")
                .arg("gh")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !on_path {
                logger::info("[gh-integration] gh CLI not found on PATH");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotInstalled));
                return;
            }
            // Step 2: Is `gh` authenticated?
            let authed = std::process::Command::new("gh")
                .args(["auth", "status"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !authed {
                logger::info("[gh-integration] gh CLI found but not authenticated");
                let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::NotAuthenticated));
                return;
            }
            logger::info("[gh-integration] gh CLI available and authenticated");
            let _ = tx.send(WorkerEvent::GhStatusChecked(GhStatus::Available));
        });
    }

    pub(crate) fn update_pr_sync_sessions(&self) {
        // Load known PRs from the database so the worker can use `gh pr view`
        // for sessions that already have a persisted PR association.
        let known_prs = self.session_store.load_all_latest_prs().unwrap_or_default();
        let known_map: HashMap<String, crate::storage::StoredPr> = known_prs
            .into_iter()
            .map(|pr| (pr.session_id.clone(), pr))
            .collect();

        if let Ok(mut guard) = self.runtime.pr_sync_sessions.lock() {
            *guard = self
                .git
                .sessions
                .iter()
                .map(|s| PrSyncEntry {
                    session_id: s.id.clone(),
                    branch_name: s.branch_name.clone(),
                    worktree_path: s.worktree_path.clone(),
                    shared_workspace: s.shared_workspace(),
                    known_pr: known_map.get(&s.id).cloned(),
                    agent_exited: !s.state.has_pty(),
                })
                .collect();
        }
    }

    pub(crate) fn spawn_pr_sync_worker(&self) {
        let tx = self.runtime.worker_tx.clone();
        let sessions = Arc::clone(&self.runtime.pr_sync_sessions);
        let enabled = Arc::clone(&self.runtime.pr_sync_enabled);
        enabled.store(true, Ordering::Relaxed);
        thread::spawn(move || {
            let interval = Duration::from_secs(45);
            loop {
                thread::sleep(interval);
                if !enabled.load(Ordering::Relaxed) {
                    break;
                }
                let output = run_pr_sync(&sessions);
                if !output.branch_updates.is_empty()
                    && tx
                        .send(WorkerEvent::BranchSyncReady(output.branch_updates))
                        .is_err()
                {
                    break;
                }
                if !output.pr_results.is_empty()
                    && tx
                        .send(WorkerEvent::PrStatusReady(output.pr_results))
                        .is_err()
                {
                    break;
                }
            }
        });
    }

    pub(crate) fn spawn_initial_pr_refresh(&self) {
        let tx = self.runtime.worker_tx.clone();
        let sessions = Arc::clone(&self.runtime.pr_sync_sessions);
        thread::spawn(move || {
            let output = run_pr_sync(&sessions);
            if !output.branch_updates.is_empty() {
                let _ = tx.send(WorkerEvent::BranchSyncReady(output.branch_updates));
            }
            if !output.pr_results.is_empty() {
                let _ = tx.send(WorkerEvent::PrStatusReady(output.pr_results));
            }
        });
    }

    /// Trigger a one-shot PR check for a single session, unless it was checked
    /// recently (within 10 seconds). Bounded to at most 4 concurrent `gh`
    /// subprocesses so a burst of refs-watcher events can't exhaust system
    /// resources.
    pub(crate) fn spawn_pr_check_for_session(&mut self, session_id: &str) {
        const MAX_PR_CHECKS_IN_FLIGHT: usize = 4;

        if !self.runtime.github_integration_enabled
            || !matches!(self.runtime.gh_status, crate::model::GhStatus::Available)
        {
            return;
        }
        // Rate-limit: skip if checked within the last 10 seconds.
        if let Some(last) = self.runtime.pr_last_checked.get(session_id)
            && last.elapsed() < Duration::from_secs(10)
        {
            return;
        }
        if self.runtime.pr_checks_in_flight.load(Ordering::Relaxed) >= MAX_PR_CHECKS_IN_FLIGHT {
            tracing::debug!(
                target: "dux::workers",
                session_id = %session_id,
                max = MAX_PR_CHECKS_IN_FLIGHT,
                "skipping PR check — too many in flight",
            );
            return;
        }
        let Some(session) = self.git.sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        let entries = if session.shared_workspace() {
            let known_prs = self.session_store.load_all_latest_prs().unwrap_or_default();
            let known_map: HashMap<String, crate::storage::StoredPr> = known_prs
                .into_iter()
                .map(|pr| (pr.session_id.clone(), pr))
                .collect();
            self.git
                .sessions
                .iter()
                .filter(|candidate| candidate.shared_workspace())
                .map(|candidate| PrSyncEntry {
                    session_id: candidate.id.clone(),
                    branch_name: candidate.branch_name.clone(),
                    worktree_path: candidate.worktree_path.clone(),
                    shared_workspace: true,
                    known_pr: known_map.get(&candidate.id).cloned(),
                    agent_exited: !candidate.state.has_pty(),
                })
                .collect()
        } else {
            let known_pr = self
                .session_store
                .load_prs(session_id)
                .ok()
                .and_then(|prs| prs.into_iter().next());
            vec![PrSyncEntry {
                session_id: session.id.clone(),
                branch_name: session.branch_name.clone(),
                worktree_path: session.worktree_path.clone(),
                shared_workspace: false,
                known_pr,
                agent_exited: !self.session_has_pty(session_id),
            }]
        };
        let tx = self.runtime.worker_tx.clone();
        let in_flight = Arc::clone(&self.runtime.pr_checks_in_flight);
        in_flight.fetch_add(1, Ordering::Relaxed);
        thread::spawn(move || {
            let output = run_pr_sync_snapshot_with(
                &entries,
                git::head_branch,
                check_pr_for_entry,
                check_pr_for_shared_entry,
            );
            if !output.branch_updates.is_empty() {
                let _ = tx.send(WorkerEvent::BranchSyncReady(output.branch_updates));
            }
            if !output.pr_results.is_empty() {
                let _ = tx.send(WorkerEvent::PrStatusReady(output.pr_results));
            }
            in_flight.fetch_sub(1, Ordering::Relaxed);
        });
    }

    pub(crate) fn spawn_changed_files_poller(&self) {
        let tx = self.runtime.worker_tx.clone();
        let watched = Arc::clone(&self.runtime.watched_worktree);
        let has_agent = Arc::clone(&self.runtime.has_active_processes);
        let shutdown = Arc::clone(&self.runtime.shutdown);
        thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                let interval = if has_agent.load(Ordering::Relaxed) {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(10)
                };
                thread::sleep(interval);
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                let path = watched.lock().ok().and_then(|guard| guard.clone());
                if let Some(worktree_path) = path
                    && let Ok((staged, unstaged)) = git::changed_files(&worktree_path)
                    && tx
                        .send(WorkerEvent::ChangedFilesReady { staged, unstaged })
                        .is_err()
                {
                    break; // receiver dropped, app is shutting down
                }
            }
        });
    }

    fn retry_hung_resume_sessions(&mut self) {
        let mut hung = Vec::new();

        for (session_id, started_at) in &self.git.resume_fallback_candidates {
            let Some(session) = self.git.sessions.iter().find(|s| s.id == *session_id) else {
                continue;
            };
            let cfg = provider_config(&self.config, &session.provider);
            let Some(timeout_ms) = cfg.resume_wait_timeout_ms.filter(|timeout| *timeout > 0) else {
                continue;
            };
            if started_at.elapsed() < Duration::from_millis(timeout_ms) {
                continue;
            }
            let Some(provider) = self.find_pty_handle(session_id) else {
                continue;
            };
            if provider.has_output() {
                continue;
            }
            hung.push(session_id.clone());
        }

        for session_id in hung {
            self.git.resume_fallback_candidates.remove(&session_id);
            let Some(session) = self
                .git
                .sessions
                .iter()
                .find(|s| s.id == session_id)
                .cloned()
            else {
                continue;
            };
            // Drop the previous PTY (kills child + joins reader).
            let _ = self.take_session_pty(&session_id);
            self.runtime.running_provider_pins.remove(&session_id);
            self.git.last_pty_activity.remove(&session_id);
            logger::info(&format!(
                "resume args produced no visible output for agent \"{}\" within timeout, retrying with regular args",
                session.branch_name
            ));
            let proj_name = self.project_name_for_session(&session);
            self.launch_fresh_with_capture(
                session.clone(),
                FreshLaunchContext {
                    success_message: format!(
                        "Resume timed out for agent \"{}\" with no visible output. Started a fresh {} session in project \"{}\".",
                        session.branch_name,
                        session.provider.as_str(),
                        proj_name,
                    ),
                    failure_prefix: format!(
                        "Timeout fallback PTY spawn failed for agent \"{}\"",
                        session.branch_name
                    ),
                    show_agent_surface: false,
                },
            );
        }
    }

    fn apply_branch_sync_updates(&mut self, updates: Vec<(String, String)>) {
        let mut changed = false;
        for (session_id, actual_branch) in updates {
            let Some(index) = self
                .git
                .sessions
                .iter()
                .position(|session| session.id == session_id)
            else {
                continue;
            };
            if self.git.sessions[index].branch_name == actual_branch {
                continue;
            }
            let mut candidate = self.git.sessions[index].clone();
            candidate.branch_name = actual_branch;
            candidate.updated_at = Utc::now();
            if let Err(err) = self.session_store.upsert_session(&candidate) {
                self.set_error(format!(
                    "Couldn't persist branch sync for session {}: {err}",
                    crate::sanitize::for_terminal(&session_id)
                ));
                continue;
            }
            tracing::info!(
                target: "dux::workers",
                session_id = %crate::sanitize::for_terminal(&session_id),
                old_branch = %crate::sanitize::for_terminal(&self.git.sessions[index].branch_name),
                new_branch = %crate::sanitize::for_terminal(&candidate.branch_name),
                "session branch synchronized",
            );
            self.git.sessions[index].branch_name = candidate.branch_name;
            self.git.sessions[index].updated_at = candidate.updated_at;
            changed = true;
        }
        if changed {
            self.update_branch_sync_sessions();
            self.rebuild_left_items();
        }
    }

    pub(crate) fn handle_prepared_fresh_launch(
        &mut self,
        session: AgentSession,
        context: FreshLaunchContext,
        result: Result<crate::resume_recovery::FreshCapture, String>,
    ) {
        self.git.fresh_launches_in_flight.remove(&session.id);
        let capture = match result {
            Ok(capture) => capture,
            Err(err) => {
                tracing::warn!(
                    target: "dux::resume_recovery",
                    session_id = %crate::sanitize::for_terminal(&session.id),
                    provider = %crate::sanitize::for_terminal(session.provider.as_str()),
                    error = %crate::sanitize::for_terminal(&err),
                    "fresh-session capture preparation failed",
                );
                self.set_error(format!("{}: {err}", context.failure_prefix));
                return;
            }
        };

        self.sync_provider_session_ids(&session);
        if self.session_has_pty(&session.id) {
            capture.abort();
            return;
        }

        match self.spawn_pty_for_session(
            &session,
            &SessionLaunch::Fresh,
            capture.claude_session_id(),
        ) {
            Ok(client) => {
                self.install_pty_for_session(&session.id, crate::pty::PtyHandle::new(client));
                self.mark_session_provider_started(&session.id);
                self.finish_fresh_capture(&session.id, capture);
                if context.show_agent_surface {
                    self.show_agent_surface();
                    self.ui.input_target = InputTarget::Agent;
                    self.ui.fullscreen_overlay = FullscreenOverlay::Agent;
                }
                if let Some(warning) = self.shared_targeted_resume_warning(&session.id) {
                    self.set_warning(warning);
                } else {
                    self.set_info(context.success_message);
                }
            }
            Err(err) => {
                capture.abort();
                tracing::error!(
                    target: "dux::resume_recovery",
                    session_id = %crate::sanitize::for_terminal(&session.id),
                    provider = %crate::sanitize::for_terminal(session.provider.as_str()),
                    error = %crate::sanitize::for_terminal(&format!("{err:#}")),
                    "fresh PTY spawn failed after capture preparation",
                );
                self.set_error(format!("{}: {err}", context.failure_prefix));
            }
        }
    }

    pub(crate) fn launch_fresh_with_capture(
        &mut self,
        session: AgentSession,
        context: FreshLaunchContext,
    ) {
        if !self.git.fresh_launches_in_flight.insert(session.id.clone()) {
            self.set_warning(format!(
                "A fresh {} session is already being prepared for agent \"{}\".",
                session.provider.as_str(),
                self.session_label(&session),
            ));
            return;
        }
        if matches!(session.provider.as_str(), "claude" | "codex") {
            let _ = dispatch_fresh_launch_preparation(
                self.runtime.worker_tx.clone(),
                self.session_store.clone(),
                session,
                context,
            );
        } else {
            self.handle_prepared_fresh_launch(
                session,
                context,
                Ok(crate::resume_recovery::FreshCapture::None),
            );
        }
    }

    pub(crate) fn finish_fresh_capture(
        &mut self,
        session_id: &str,
        capture: crate::resume_recovery::FreshCapture,
    ) {
        let process_id = self
            .find_pty_handle(session_id)
            .and_then(|pty| pty.child_process_id());
        match capture {
            crate::resume_recovery::FreshCapture::None => {}
            crate::resume_recovery::FreshCapture::Claude {
                session_id: provider_session_id,
                persist_after_spawn,
            } => {
                if persist_after_spawn {
                    dispatch_provider_session_id_persist(
                        self.runtime.worker_tx.clone(),
                        self.session_store.clone(),
                        session_id.to_string(),
                        "claude".to_string(),
                        provider_session_id,
                    );
                } else if let Some(session) = self
                    .git
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                {
                    session
                        .provider_session_ids
                        .insert("claude".to_string(), provider_session_id);
                }
            }
            crate::resume_recovery::FreshCapture::Codex(capture) => {
                dispatch_codex_session_capture(
                    self.runtime.worker_tx.clone(),
                    self.session_store.clone(),
                    session_id.to_string(),
                    capture,
                    process_id,
                );
            }
        }
    }

    pub(crate) fn sync_provider_session_ids(&mut self, prepared: &AgentSession) {
        if let Some(session) = self
            .git
            .sessions
            .iter_mut()
            .find(|session| session.id == prepared.id)
        {
            session.provider_session_ids = prepared.provider_session_ids.clone();
        }
    }

    pub(crate) fn shared_targeted_resume_warning(&self, session_id: &str) -> Option<String> {
        let session = self
            .git
            .sessions
            .iter()
            .find(|session| session.id == session_id)?;
        if !session.shared_workspace()
            || provider_config(&self.config, &session.provider).supports_session_resume_by_id()
        {
            return None;
        }
        Some(format!(
            "Started shared {} agent \"{}\" fresh. Exact per-agent resume is unavailable because providers.{}.resume_by_id_args is not configured; dux will never use a latest-session selector in shared mode.",
            session.provider.as_str(),
            self.session_label(session),
            session.provider.as_str(),
        ))
    }

    fn handle_provider_session_captured(
        &mut self,
        session_id: &str,
        provider: &str,
        result: Result<String, String>,
    ) {
        match result {
            Ok(provider_session_id) => {
                if let Some(session) = self
                    .git
                    .sessions
                    .iter_mut()
                    .find(|session| session.id == session_id)
                {
                    session
                        .provider_session_ids
                        .insert(provider.to_string(), provider_session_id);
                }
                tracing::info!(
                    target: "dux::resume_recovery",
                    session_id = %crate::sanitize::for_terminal(session_id),
                    provider = %crate::sanitize::for_terminal(provider),
                    "provider session UUID captured and persisted",
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "dux::resume_recovery",
                    session_id = %crate::sanitize::for_terminal(session_id),
                    provider = %crate::sanitize::for_terminal(provider),
                    error = %crate::sanitize::for_terminal(&err),
                    "provider session UUID capture failed closed",
                );
                if provider == "codex" {
                    self.set_warning(format!(
                        "Could not capture the exact Codex conversation for this agent: {err}. Another uncaptured Codex launch in this workspace is blocked until dux restarts."
                    ));
                } else {
                    self.set_warning(format!(
                        "Could not persist the replacement {provider} conversation UUID: {err}. The prior UUID was retained; retry a fresh restart."
                    ));
                }
            }
        }
    }

    fn handle_resume_recovery_completed(
        &mut self,
        result: Result<crate::resume_recovery::RecoveryReport, String>,
    ) {
        match result {
            Ok(report) => {
                for update in &report.updates {
                    if let Some(session) = self
                        .git
                        .sessions
                        .iter_mut()
                        .find(|session| session.id == update.session_id)
                    {
                        session
                            .provider_session_ids
                            .insert(update.provider.clone(), update.provider_session_id.clone());
                    }
                }
                tracing::info!(
                    target: "dux::resume_recovery",
                    recovered_sessions = report.updates.len(),
                    copied_artifacts = report.copied_artifacts,
                    warning_count = report.warnings.len(),
                    "provider session recovery completed",
                );
                for warning in &report.warnings {
                    tracing::warn!(
                        target: "dux::resume_recovery",
                        warning = %crate::sanitize::for_terminal(warning),
                        "provider session recovery warning",
                    );
                }
                if let Some(warning) = report.warnings.first() {
                    self.set_warning(warning.clone());
                }
            }
            Err(err) => {
                tracing::warn!(
                    target: "dux::resume_recovery",
                    error = %crate::sanitize::for_terminal(&err),
                    "provider session recovery failed; continuing startup",
                );
                self.set_warning(format!(
                    "Existing provider conversation recovery did not complete: {err}"
                ));
            }
        }
        self.auto_resume_all_sessions();
    }

    pub(crate) fn queue_config_save(
        &self,
        success: impl Into<String>,
        rollback: ConfigSaveRollback,
    ) {
        dispatch_config_save(
            self.runtime.worker_tx.clone(),
            self.paths.config_path.clone(),
            self.config.clone(),
            success.into(),
            rollback,
        );
    }
}

pub(crate) fn dispatch_fresh_launch_preparation(
    tx: Sender<WorkerEvent>,
    store: SessionStore,
    mut session: AgentSession,
    context: FreshLaunchContext,
) -> Result<()> {
    let failure_tx = tx.clone();
    let failure_session = session.clone();
    let failure_context = context.clone();
    thread::Builder::new()
        .name("provider-session-prepare".to_string())
        .spawn(move || {
            let result =
                crate::resume_recovery::prepare_fresh_capture_from_home(&mut session, &store)
                    .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::FreshLaunchPrepared {
                session: Box::new(session),
                context,
                result,
            });
        })
        .map(|_| ())
        .map_err(|err| {
            let message = format!("could not start fresh-session preparation worker: {err}");
            let _ = failure_tx.send(WorkerEvent::FreshLaunchPrepared {
                session: Box::new(failure_session),
                context: failure_context,
                result: Err(message.clone()),
            });
            anyhow::anyhow!(message)
        })
}

fn dispatch_provider_session_id_persist(
    tx: Sender<WorkerEvent>,
    store: SessionStore,
    session_id: String,
    provider: String,
    provider_session_id: String,
) {
    let failure_tx = tx.clone();
    let failure_session_id = session_id.clone();
    let failure_provider = provider.clone();
    let spawn = thread::Builder::new()
        .name("provider-session-persist".to_string())
        .spawn(move || {
            let result = store
                .set_provider_session_id(&session_id, &provider, &provider_session_id)
                .map(|()| provider_session_id)
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::ProviderSessionCaptured {
                session_id,
                provider,
                result,
            });
        });
    if let Err(err) = spawn {
        let _ = failure_tx.send(WorkerEvent::ProviderSessionCaptured {
            session_id: failure_session_id,
            provider: failure_provider,
            result: Err(format!(
                "could not start provider-session persistence worker: {err}"
            )),
        });
    }
}

fn dispatch_codex_session_capture(
    tx: Sender<WorkerEvent>,
    store: SessionStore,
    session_id: String,
    capture: crate::resume_recovery::CodexCapture,
    process_id: Option<u32>,
) {
    let failure_tx = tx.clone();
    let failure_session_id = session_id.clone();
    let spawn = thread::Builder::new()
        .name("codex-session-capture".to_string())
        .spawn(move || {
            let result = match capture.wait_for_id(None, process_id) {
                Ok(Some(provider_session_id)) => {
                    match store.set_provider_session_id(&session_id, "codex", &provider_session_id)
                    {
                        Ok(()) => {
                            capture.resolve();
                            Ok(provider_session_id)
                        }
                        Err(err) => {
                            let message = format!("failed to persist captured Codex UUID: {err:#}");
                            capture.block(&message);
                            Err(message)
                        }
                    }
                }
                Ok(None) => {
                    capture.abort();
                    return;
                }
                Err(err) => {
                    let message = format!("{err:#}");
                    capture.block(&message);
                    Err(message)
                }
            };
            let _ = tx.send(WorkerEvent::ProviderSessionCaptured {
                session_id,
                provider: "codex".to_string(),
                result,
            });
        });
    if let Err(err) = spawn {
        // Dropping the still-active capture fails closed in its coordinator.
        let _ = failure_tx.send(WorkerEvent::ProviderSessionCaptured {
            session_id: failure_session_id,
            provider: "codex".to_string(),
            result: Err(format!("could not start Codex capture worker: {err}")),
        });
    }
}

pub(crate) fn dispatch_resume_recovery(
    tx: Sender<WorkerEvent>,
    sessions: Vec<AgentSession>,
    projects: Vec<Project>,
    worktrees_root: PathBuf,
    store: SessionStore,
) -> Result<()> {
    let failure_tx = tx.clone();
    thread::Builder::new()
        .name("provider-session-recovery".to_string())
        .spawn(move || {
            let result = crate::resume_recovery::ProviderDataRoots::from_home()
                .and_then(|roots| {
                    crate::resume_recovery::recover_stranded_histories(
                        &sessions,
                        &projects,
                        &worktrees_root,
                        &roots,
                        &store,
                    )
                })
                .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::ResumeRecoveryCompleted(result));
        })
        .map(|_| ())
        .map_err(|err| {
            let message = format!("could not start provider-session recovery worker: {err}");
            let _ = failure_tx.send(WorkerEvent::ResumeRecoveryCompleted(Err(message.clone())));
            anyhow::anyhow!(message)
        })
}

pub(crate) fn dispatch_orphan_worktree_inventory(tx: Sender<WorkerEvent>, paths: DuxPaths) {
    let failure_tx = tx.clone();
    let spawn = thread::Builder::new()
        .name("orphan-worktree-inventory".to_string())
        .spawn(move || {
            let result = crate::orphan_worktrees::inventory(&paths).map_err(|err| {
                tracing::warn!(
                    target: "dux::orphan_worktrees",
                    error = %crate::sanitize::for_terminal(&format!("{err:#}")),
                    "orphan worktree inventory failed closed",
                );
                format!("{err:#}")
            });
            if let Ok(candidates) = &result {
                tracing::info!(
                    target: "dux::orphan_worktrees",
                    candidates = candidates.len(),
                    "orphan worktree inventory completed",
                );
            }
            let _ = tx.send(WorkerEvent::OrphanWorktreesReady(result));
        });
    if let Err(err) = spawn {
        let _ = failure_tx.send(WorkerEvent::OrphanWorktreesReady(Err(format!(
            "failed to start orphan-worktree inventory worker: {err}"
        ))));
    }
}

pub(crate) fn dispatch_orphan_worktree_removal(
    tx: Sender<WorkerEvent>,
    paths: DuxPaths,
    candidate: crate::orphan_worktrees::OrphanWorktreeCandidate,
    candidates: Vec<crate::orphan_worktrees::OrphanWorktreeCandidate>,
    selected: usize,
    delete_branch: bool,
) {
    let event_candidate = candidate.clone();
    let failure_candidate = event_candidate.clone();
    let failure_candidates = candidates.clone();
    let failure_tx = tx.clone();
    let spawn = thread::Builder::new()
        .name("orphan-worktree-remove".to_string())
        .spawn(move || {
            let safe_path =
                crate::sanitize::for_terminal(&candidate.worktree_path.display().to_string());
            tracing::info!(
                target: "dux::orphan_worktrees",
                worktree = %safe_path,
                delete_branch,
                "revalidating orphan worktree before removal",
            );
            let result =
                crate::orphan_worktrees::remove(&paths, &candidate.worktree_path, delete_branch)
                    .map_err(|err| format!("{err:#}"));
            if let Err(err) = &result {
                tracing::warn!(
                    target: "dux::orphan_worktrees",
                    worktree = %safe_path,
                    error = %crate::sanitize::for_terminal(err),
                    "orphan worktree removal failed",
                );
            } else {
                tracing::info!(
                    target: "dux::orphan_worktrees",
                    worktree = %safe_path,
                    "orphan worktree removed",
                );
            }
            let _ = tx.send(WorkerEvent::OrphanWorktreeRemoved {
                candidate: event_candidate,
                candidates,
                selected,
                delete_branch,
                result,
            });
        });
    if let Err(err) = spawn {
        let _ = failure_tx.send(WorkerEvent::OrphanWorktreeRemoved {
            candidate: failure_candidate,
            candidates: failure_candidates,
            selected,
            delete_branch,
            result: Err(format!(
                "failed to start orphan-worktree removal worker: {err}"
            )),
        });
    }
}

/// Background job for "Add Project" when the user opted to have dux switch to
/// the default branch first. Runs `git switch <target_branch>` in the source
/// repo and reports the outcome via `WorkerEvent::AddProjectCheckoutCompleted`
/// so the main loop can either call `finish_add_project` or surface the error.
pub(crate) fn run_add_project_checkout_job(
    path: String,
    name: String,
    target_branch: String,
    worker_tx: Sender<WorkerEvent>,
) {
    let result = git::switch_branch(Path::new(&path), &target_branch).map_err(|e| format!("{e:#}"));
    let _ = worker_tx.send(WorkerEvent::AddProjectCheckoutCompleted {
        path,
        name,
        target_branch,
        result,
    });
}

pub(crate) fn run_create_agent_job(
    request: CreateAgentRequest,
    paths: DuxPaths,
    config: Config,
    store_id: String,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
) {
    let registered_project_paths = match crate::config::registered_project_paths(&config) {
        Ok(paths) => paths,
        Err(err) => {
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Project inventory is invalid: {}",
                crate::sanitize::for_terminal(&format!("{err:#}"))
            )));
            return;
        }
    };
    let (
        project,
        provider,
        settings,
        source_branch,
        mut status_message,
        branch_name,
        worktree_path,
        owns_worktree,
        owns_branch,
        requested_handle,
        shared_workspace,
    ) = match request {
        CreateAgentRequest::NewProject {
            project,
            custom_name,
            use_existing_branch,
            provider,
            settings,
        } => {
            let repo_path = PathBuf::from(&project.path);

            // Resolve the branch name early so we can check for an
            // existing branch before calling git worktree add.  When no
            // custom name was provided, a random pet name is generated.
            let resolved_name = custom_name.unwrap_or_else(git::docker_style_name);

            // If the caller already confirmed via the UI dialog,
            // `use_existing_branch` is true.  Otherwise, do a last-mile
            // check — this covers auto-generated pet names that
            // coincidentally match an existing branch.
            let attach_existing =
                use_existing_branch || git::branch_exists(&repo_path, &resolved_name).is_some();

            let progress = if attach_existing {
                format!(
                    "Attaching to existing branch \"{}\" for project \"{}\"...",
                    resolved_name, project.name
                )
            } else {
                format!(
                    "Creating a new worktree for project \"{}\"...",
                    project.name
                )
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(progress));

            let (branch_name, worktree_path) = if attach_existing {
                match git::create_worktree_existing_branch(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    &resolved_name,
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        logger::error(&format!(
                            "worktree creation (existing branch) failed for {}: {err}",
                            project.path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                            "Failed to attach to existing branch for project \"{}\": {err}",
                            project.name
                        )));
                        return;
                    }
                }
            } else {
                match git::create_worktree(
                    &repo_path,
                    &paths.worktrees_root,
                    &project.name,
                    Some(&resolved_name),
                ) {
                    Ok(result) => result,
                    Err(err) => {
                        logger::error(&format!(
                            "worktree creation failed for {}: {err}",
                            project.path
                        ));
                        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                            "Failed to create a new worktree for project \"{}\": {err}",
                            project.name
                        )));
                        return;
                    }
                }
            };
            let status_message = if attach_existing {
                format!(
                    "Attached to existing branch \"{}\" in project \"{}\". The worktree is ready in a fresh session.",
                    branch_name, project.name
                )
            } else {
                format!(
                    "Created {} agent \"{}\" in project \"{}\". The new worktree is ready in a fresh session.",
                    provider.as_str(),
                    branch_name,
                    project.name
                )
            };
            (
                project.clone(),
                provider,
                settings,
                project.current_branch.clone(),
                status_message,
                branch_name,
                worktree_path,
                true,
                !attach_existing,
                None,
                false,
            )
        }
        CreateAgentRequest::SharedWorkspace {
            project,
            agent_handle,
            provider,
            settings,
        } => {
            if let Err(err) = crate::config::validate_shared_workspace_path(&project.path, &paths) {
                let safe_err = crate::sanitize::for_terminal(&format!("{err:#}"));
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Shared workspace is not eligible: {safe_err}"
                )));
                return;
            }
            let handle = agent_handle.unwrap_or_else(git::docker_style_name);
            if !crate::model::is_valid_agent_handle(&handle) {
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(
                    "Shared agent handle must be 1–64 lowercase letters, digits, dashes, or underscores."
                        .to_string(),
                ));
                return;
            }
            let repo_path = PathBuf::from(&project.path);
            let safe_project_name = crate::sanitize::for_terminal(&project.name);
            let branch_name = match git::head_branch(&repo_path) {
                Ok(Some(branch)) => branch,
                Ok(None) => DETACHED_HEAD_LABEL.to_string(),
                Err(err) => {
                    let safe_err = crate::sanitize::for_terminal(&format!("{err:#}"));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to inspect the shared workspace HEAD: {safe_err}"
                    )));
                    return;
                }
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Preparing shared workspace for project \"{safe_project_name}\"..."
            )));
            let status_message = format!(
                "Created shared {} agent \"{}\" in project \"{}\". It is running in the registered checkout.",
                provider.as_str(),
                handle,
                safe_project_name
            );
            (
                project,
                provider,
                settings,
                branch_name.clone(),
                status_message,
                branch_name,
                repo_path,
                false,
                false,
                Some(handle),
                true,
            )
        }
        CreateAgentRequest::ForkSession {
            project,
            source_session,
            source_label,
            custom_name,
            provider,
            settings,
        } => {
            let source_worktree = PathBuf::from(&source_session.worktree_path);
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Creating a forked worktree from agent \"{source_label}\"...",
            )));
            let source_head = match git::head_commit(&source_worktree) {
                Ok(head) => head,
                Err(err) => {
                    logger::error(&format!(
                        "failed to resolve HEAD for {}: {err}",
                        source_session.worktree_path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to inspect the source worktree for agent \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let repo_path = PathBuf::from(&project.path);
            let (branch_name, worktree_path) = match git::create_worktree_from_start_point(
                &repo_path,
                &paths.worktrees_root,
                &project.name,
                Some(&source_head),
                custom_name.as_deref(),
            ) {
                Ok(result) => result,
                Err(err) => {
                    logger::error(&format!(
                        "fork worktree creation failed for {}: {err}",
                        project.path
                    ));
                    let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                        "Failed to create a forked worktree from agent \"{source_label}\": {err}",
                    )));
                    return;
                }
            };
            let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
                "Copying the current filesystem contents from agent \"{source_label}\" into the new fork...",
            )));
            if let Err(err) = git::mirror_worktree_contents(&source_worktree, &worktree_path) {
                logger::error(&format!(
                    "failed to mirror worktree {} into {}: {err}",
                    source_worktree.display(),
                    worktree_path.display()
                ));
                let _ = git::remove_worktree(
                    &repo_path,
                    &worktree_path,
                    &branch_name,
                    true,
                    &registered_project_paths,
                );
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Failed to copy the source worktree contents for agent \"{source_label}\": {err}",
                )));
                return;
            }
            let status_message = format!(
                "Forked {} agent \"{}\" from \"{}\" in project \"{}\". The new worktree starts with copied files and a fresh session.",
                provider.as_str(),
                branch_name,
                source_label,
                project.name
            );
            (
                project,
                provider,
                settings,
                source_session.branch_name,
                status_message,
                branch_name,
                worktree_path,
                true,
                true,
                None,
                false,
            )
        }
    };
    if shared_workspace {
        tracing::info!(
            target: "dux::workers",
            project_id = %crate::sanitize::for_terminal(&project.id),
            workspace = %crate::sanitize::for_terminal(&worktree_path.display().to_string()),
            branch = %crate::sanitize::for_terminal(&branch_name),
            "using registered checkout for shared session",
        );
    } else if owns_worktree {
        logger::info(&format!(
            "created worktree {} on branch {}",
            worktree_path.display(),
            branch_name
        ));
    } else {
        logger::info(&format!(
            "reusing worktree {} on branch {} for new provider session",
            worktree_path.display(),
            branch_name
        ));
    }
    let id = Uuid::new_v4().to_string();
    let worktree_path_string = worktree_path.to_string_lossy().to_string();
    let agent_handle = requested_handle.unwrap_or_else(|| {
        crate::model::derive_agent_handle(&worktree_path_string, &branch_name, &id)
    });
    let mut session = AgentSession {
        id,
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider,
        source_branch,
        branch_name,
        worktree_path: worktree_path_string,
        agent_handle,
        shared_workspace,
        deleted_at: None,
        title: None,
        started_providers: Vec::new(),
        provider_session_ids: Default::default(),
        state: crate::model::SessionState::Spawning { since: Utc::now() },
        settings,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let store = match SessionStore::open(&paths.sessions_db_path) {
        Ok(store) => store,
        Err(err) => {
            let safe_err = crate::sanitize::for_terminal(&format!("{err:#}"));
            if owns_worktree {
                let _ = git::remove_worktree(
                    Path::new(&project.path),
                    Path::new(&session.worktree_path),
                    &session.branch_name,
                    owns_branch,
                    &registered_project_paths,
                );
            }
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Failed to open the session store: {safe_err}"
            )));
            return;
        }
    };
    if let Err(err) =
        crate::peer::reserve_and_persist_session(&paths, &store_id, &store, &mut session)
    {
        let safe_err = crate::sanitize::for_terminal(&format!("{err:#}"));
        let row_exists = store
            .load_sessions_including_deleted()
            .is_ok_and(|rows| rows.iter().any(|row| row.id == session.id));
        if row_exists {
            fail_recoverable_create(
                &store,
                session,
                format!("Session reserved but AMQ setup failed: {safe_err}"),
                &worker_tx,
            );
        } else {
            if owns_worktree {
                let _ = git::remove_worktree(
                    Path::new(&project.path),
                    Path::new(&session.worktree_path),
                    &session.branch_name,
                    owns_branch,
                    &registered_project_paths,
                );
            }
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Failed to persist the new session: {safe_err}"
            )));
        }
        return;
    }
    if session.shared_workspace() {
        session.title = Some(session.agent_handle().to_string());
        session.updated_at = Utc::now();
        if let Err(err) = store.upsert_session(&session) {
            let safe_err = crate::sanitize::for_terminal(&format!("{err:#}"));
            fail_recoverable_create(
                &store,
                session,
                format!(
                    "Shared session reserved but its display title could not be saved: {safe_err}"
                ),
                &worker_tx,
            );
            return;
        }
        status_message = format!(
            "Created shared {} agent \"{}\" in project \"{}\". It is running in the registered checkout.",
            session.provider.as_str(),
            session.agent_handle(),
            crate::sanitize::for_terminal(&project.name)
        );
    }
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(&provider_cfg) {
        tracing::error!(
            target: "dux::workers",
            session_id = %session.id,
            provider = %session.provider.as_str(),
            command = %provider_cfg.command,
            hint = %hint,
            "provider not found",
        );
        fail_recoverable_create(&store, session, hint, &worker_tx);
        return;
    }
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
        "Launching {} in a fresh session...",
        session.provider.as_str()
    )));
    let fresh_capture =
        match crate::resume_recovery::prepare_fresh_capture_from_home(&mut session, &store) {
            Ok(capture) => capture,
            Err(err) => {
                let safe_err = crate::sanitize::for_terminal(&format!("{err:#}"));
                fail_recoverable_create(
                    &store,
                    session,
                    format!(
                        "Failed to prepare exact {} conversation capture: {safe_err}",
                        provider_cfg.command
                    ),
                    &worker_tx,
                );
                return;
            }
        };
    let launch_args = match sessions::launch_args(
        &provider_cfg,
        &session.provider,
        &SessionLaunch::Fresh,
        fresh_capture.claude_session_id(),
        session.settings.yolo_permissions,
        &worktree_path,
    ) {
        Ok(args) => args,
        Err(err) => {
            fresh_capture.abort();
            fail_recoverable_create(
                &store,
                session,
                format!("Failed to build provider launch arguments: {err}"),
                &worker_tx,
            );
            return;
        }
    };
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    // audit03 Phase 3: thread per-session env (YOLO, verify
    // override) into the spawn so the wrappers see deterministic
    // CLI flags. `verify_envelope_override.is_none()` falls through
    // to the global config value here, where the `Config` is in scope.
    let mut per_session_env = session
        .settings
        .to_pty_env(&session.provider, config.amq.inject.verify_envelope);
    crate::peer::append_session_env(&mut per_session_env, &session, &store_id);
    let client = match PtyClient::spawn_with_env(
        &provider_cfg.command,
        &launch_args,
        &worktree_path,
        rows,
        cols,
        config.ui.agent_scrollback_lines,
        per_session_env,
    ) {
        Ok(client) => client,
        Err(err) => {
            fresh_capture.abort();
            tracing::error!(
                target: "dux::workers",
                session_id = %session.id,
                command = %provider_cfg.command,
                worktree = %crate::sanitize::for_terminal(&worktree_path.display().to_string()),
                err = %err,
                "pty spawn failed",
            );
            fail_recoverable_create(
                &store,
                session,
                format!("Failed to start {}: {err}", provider_cfg.command),
                &worker_tx,
            );
            return;
        }
    };
    tracing::info!(
        target: "dux::workers",
        session_id = %session.id,
        provider = %session.provider.as_str(),
        rows = rows,
        cols = cols,
        "pty session started",
    );
    let _ = worker_tx.send(WorkerEvent::CreateAgentReady(Box::new(AgentReadyData {
        session,
        client,
        pty_size: (rows, cols),
        status_message,
        fresh_capture,
    })));
}

fn fail_recoverable_create(
    store: &SessionStore,
    mut session: AgentSession,
    message: String,
    worker_tx: &Sender<WorkerEvent>,
) {
    session.state = SessionState::Retryable {
        interrupted_at: Utc::now(),
    };
    session.updated_at = Utc::now();
    if let Err(err) = store.upsert_session(&session) {
        tracing::error!(
            target: "dux::workers",
            session_id = %crate::sanitize::for_terminal(&session.id),
            err = %crate::sanitize::for_terminal(&format!("{err:#}")),
            "failed to persist retryable create state"
        );
    }
    let _ = worker_tx.send(WorkerEvent::CreateAgentRecoverable {
        session: Box::new(session),
        message,
    });
}

/// Fan out `git is_git_repo` + `current_branch` + `remote_default_branch`
/// for each provided project path. Each path runs in its own thread, so a
/// stuck git process for one project cannot delay the rest. Results land
/// asynchronously on the worker channel as
/// [`WorkerEvent::ProjectMetaReady`]; the main loop uses them to fill in
/// the placeholder `Project` rows produced by `load_projects`.
///
/// Spawning N short-lived threads is acceptable here — N is typically <20
/// (the number of projects in `config.toml`) and the alternative (a single
/// serial thread) would re-introduce the head-of-line blocking the original
/// synchronous code suffered from.
pub(crate) fn dispatch_project_meta(tx: Sender<WorkerEvent>, paths: Vec<PathBuf>) {
    for path in paths {
        let tx = tx.clone();
        let _ = thread::Builder::new()
            .name(format!("project-meta-{}", path.display()))
            .spawn(move || {
                let exists = path.exists();
                let is_git = exists && git::is_git_repo(&path);
                let head = if is_git {
                    git::head_branch(&path).ok()
                } else {
                    None
                };
                let current_branch = head.as_ref().and_then(|branch| branch.clone());
                let head_detached = matches!(head, Some(None));
                let remote_default = if is_git {
                    git::remote_default_branch(&path)
                } else {
                    None
                };
                let _ = tx.send(WorkerEvent::ProjectMetaReady {
                    path,
                    is_git,
                    current_branch,
                    head_detached,
                    remote_default,
                });
            });
    }
}

/// Run `git status --porcelain` against `worktree` on a worker thread and
/// send the result back as [`WorkerEvent::ReloadChangedFilesReady`]. Used
/// by `App::reload_changed_files`, which used to block the UI thread.
///
/// The reply carries the worktree path so out-of-order replies (rapid
/// session switching) can be discarded by the main loop without clobbering
/// the currently-selected pane.
pub(crate) fn dispatch_changed_files(tx: Sender<WorkerEvent>, worktree: PathBuf) {
    let _ = thread::Builder::new()
        .name(format!("changed-files-{}", worktree.display()))
        .spawn(move || {
            let result = git::changed_files(&worktree).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerEvent::ReloadChangedFilesReady { worktree, result });
        });
}

/// Run `git diff --cached` against `worktree` on a worker thread and send
/// the result back as [`WorkerEvent::StagedDiffReady`]. Used by the AI
/// commit-message generator, which used to call `git::staged_diff_text`
/// inline before spawning the provider thread.
pub(crate) fn dispatch_staged_diff(tx: Sender<WorkerEvent>, worktree: PathBuf) {
    let _ = thread::Builder::new()
        .name(format!("staged-diff-{}", worktree.display()))
        .spawn(move || {
            let result = git::staged_diff_text(&worktree).map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerEvent::StagedDiffReady { worktree, result });
        });
}

/// Run `git commit -m <message>` against `worktree` on a worker thread.
/// The caller is responsible for blocking input via `PromptState::Busy*`
/// before dispatching, and for re-enabling input in the event handler so
/// the UI stays responsive (and ordering stays correct).
pub(crate) fn dispatch_commit(tx: Sender<WorkerEvent>, worktree: PathBuf, message: String) {
    let _ = thread::Builder::new()
        .name(format!("commit-{}", worktree.display()))
        .spawn(move || {
            let result = git::commit(&worktree, &message)
                .map(|_| ())
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(WorkerEvent::CommitFinished {
                worktree,
                message,
                result,
            });
        });
}

pub(crate) fn dispatch_git_file_operation(
    tx: Sender<WorkerEvent>,
    action: GitFileAction,
    worktree: PathBuf,
    path: String,
) {
    let _ = thread::Builder::new()
        .name(format!("git-file-{}", worktree.display()))
        .spawn(move || {
            let result = match action {
                GitFileAction::Stage => git::stage_file(&worktree, &path),
                GitFileAction::Unstage => git::unstage_file(&worktree, &path),
                GitFileAction::Discard { untracked } => {
                    git::discard_file(&worktree, &path, untracked)
                }
            }
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::GitFileOperationCompleted {
                action,
                worktree,
                path,
                result,
            });
        });
}

/// Decide whether a completed stage/unstage should move the right-pane focus
/// off a section it just emptied. `staged`/`unstaged` are the list lengths
/// *before* the operation is reflected, so a source section with a single
/// entry is about to become empty. Returns `None` to keep the current section.
fn section_after_git_file_op(
    action: &GitFileAction,
    staged: usize,
    unstaged: usize,
) -> Option<RightSection> {
    match action {
        GitFileAction::Stage if unstaged <= 1 && staged > 0 => Some(RightSection::Staged),
        GitFileAction::Unstage if staged <= 1 && unstaged > 0 => Some(RightSection::Unstaged),
        _ => None,
    }
}

fn dispatch_branch_rename_rollback(
    tx: Sender<WorkerEvent>,
    session_id: String,
    worktree: String,
    renamed_branch: String,
    original_branch: String,
) {
    let _ = thread::Builder::new()
        .name("branch-rename-rollback".to_string())
        .spawn(move || {
            let result =
                git::rename_branch(Path::new(&worktree), &renamed_branch, &original_branch)
                    .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::BranchRenameRollbackCompleted { session_id, result });
        });
}

pub(crate) fn dispatch_path_completions(tx: Sender<WorkerEvent>, query: String, reverse: bool) {
    let _ = thread::Builder::new()
        .name("path-completions".to_string())
        .spawn(move || {
            let candidates = path_completions(&query);
            let _ = tx.send(WorkerEvent::PathCompletionsReady {
                query,
                reverse,
                candidates,
            });
        });
}

fn path_completions(query: &str) -> Vec<String> {
    let input_path = PathBuf::from(query);
    let (search_dir, prefix) = if input_path.is_dir() && query.ends_with('/') {
        (input_path, String::new())
    } else {
        (
            input_path
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .to_path_buf(),
            input_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    let prefix = prefix.to_lowercase();
    let mut candidates = fs::read_dir(&search_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            !name.starts_with('.') && name.starts_with(&prefix)
        })
        .map(|entry| {
            let mut full = search_dir
                .join(entry.file_name())
                .to_string_lossy()
                .into_owned();
            full.push('/');
            full
        })
        .collect::<Vec<_>>();
    candidates.sort();
    candidates
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_diff(
    tx: Sender<WorkerEvent>,
    worktree: String,
    rel_path: String,
    theme: Theme,
    show_line_numbers: bool,
    tab_width: u16,
    scroll: u16,
    focus_when_ready: bool,
) {
    let _ = thread::Builder::new()
        .name(format!("diff-{rel_path}"))
        .spawn(move || {
            // ponytail: per-diff SyntaxCache — the shared App cache was
            // dropped so diff work could move off the UI thread (P1-23).
            // Cross-diff syntax reuse is lost; if diff latency matters,
            // give the worker a persistent Arc<Mutex<SyntaxCache>>.
            let cache = crate::diff::SyntaxCache::new();
            let result = crate::diff::diff_file(
                Path::new(&worktree),
                &rel_path,
                &theme,
                &cache,
                show_line_numbers,
                tab_width,
            )
            .map_err(|err| format!("{err:#}"));
            let _ = tx.send(WorkerEvent::DiffReady {
                worktree,
                rel_path,
                scroll,
                focus_when_ready,
                result,
            });
        });
}

struct ConfigSaveJob {
    event_tx: Sender<WorkerEvent>,
    path: PathBuf,
    config: Config,
    success: String,
    rollback: ConfigSaveRollback,
}

enum ConfigWorkerJob {
    Save(Box<ConfigSaveJob>),
    Flush(Sender<()>),
}

static CONFIG_SAVE_TX: std::sync::OnceLock<Option<Sender<ConfigWorkerJob>>> =
    std::sync::OnceLock::new();

fn config_save_tx() -> Option<&'static Sender<ConfigWorkerJob>> {
    CONFIG_SAVE_TX
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<ConfigWorkerJob>();
            match thread::Builder::new()
                .name("config-save".to_string())
                .spawn(move || run_config_save_worker(rx))
            {
                Ok(_) => Some(tx),
                Err(err) => {
                    tracing::error!(
                        target: "dux::workers",
                        err = %crate::sanitize::for_terminal(&err.to_string()),
                        "failed to spawn config-save worker; using synchronous persistence",
                    );
                    None
                }
            }
        })
        .as_ref()
}

fn run_config_save_worker(rx: mpsc::Receiver<ConfigWorkerJob>) {
    let mut last_requested = HashMap::<PathBuf, Config>::new();
    for job in rx {
        match job {
            ConfigWorkerJob::Save(job) => {
                let previous = last_requested.insert(job.path.clone(), job.config.clone());
                let result = save_config_change(&job.path, previous.as_ref(), &job.config)
                    .map_err(|err| crate::sanitize::for_terminal(&format!("{err:#}")));
                let _ = job.event_tx.send(WorkerEvent::ConfigSaveCompleted {
                    success: job.success,
                    rollback: job.rollback,
                    result,
                });
            }
            ConfigWorkerJob::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

fn save_config_change(path: &Path, previous: Option<&Config>, desired: &Config) -> Result<()> {
    #[cfg(test)]
    if let Some(action) = take_config_save_test_action(path) {
        match action {
            ConfigSaveTestAction::Delay(delay) => thread::sleep(delay),
            ConfigSaveTestAction::Fail(message) => anyhow::bail!(message),
        }
    }

    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err.into()),
    };
    let mut on_disk = if raw.is_empty() {
        Config::default()
    } else {
        let parsed: Config = toml::from_str(&raw)?;
        let mut config = crate::config::migrate_config(parsed);
        config.providers.ensure_defaults();
        config
    };

    let baseline = previous.unwrap_or(&on_disk);
    let baseline = toml::Value::try_from(baseline)?;
    let desired = toml::Value::try_from(desired)?;
    let mut patched = toml::Value::try_from(&on_disk)?;
    apply_toml_delta(&baseline, &desired, &mut patched);
    on_disk = patched.try_into()?;

    let bindings = RuntimeBindings::from_keys_config(&on_disk.keys);
    crate::config::save_config(path, &on_disk, &bindings)
}

fn apply_toml_delta(previous: &toml::Value, desired: &toml::Value, target: &mut toml::Value) {
    if previous == desired {
        return;
    }
    if let (toml::Value::Table(previous), toml::Value::Table(desired), toml::Value::Table(target)) =
        (previous, desired, &mut *target)
    {
        target.retain(|key, _| desired.contains_key(key) || !previous.contains_key(key));
        for (key, desired_value) in desired {
            let previous_value = previous.get(key);
            if previous_value == Some(desired_value) {
                continue;
            }
            match (previous_value, target.get_mut(key)) {
                (Some(previous_value), Some(target_value)) => {
                    apply_toml_delta(previous_value, desired_value, target_value);
                }
                (Some(_), None) | (None, _) => {
                    target.insert(key.clone(), desired_value.clone());
                }
            }
        }
        return;
    }
    *target = desired.clone();
}

fn save_config_synchronously(job: Box<ConfigSaveJob>) {
    let bindings = RuntimeBindings::from_keys_config(&job.config.keys);
    let result = crate::config::save_config(&job.path, &job.config, &bindings)
        .map_err(|err| crate::sanitize::for_terminal(&format!("{err:#}")));
    let _ = job.event_tx.send(WorkerEvent::ConfigSaveCompleted {
        success: job.success,
        rollback: job.rollback,
        result,
    });
}

fn dispatch_config_save_job(sender: Option<&Sender<ConfigWorkerJob>>, job: Box<ConfigSaveJob>) {
    let job = if let Some(sender) = sender {
        match sender.send(ConfigWorkerJob::Save(job)) {
            Ok(()) => return,
            Err(err) => {
                let ConfigWorkerJob::Save(job) = err.0 else {
                    return;
                };
                tracing::warn!(
                    target: "dux::workers",
                    "config-save worker stopped; using synchronous persistence",
                );
                job
            }
        }
    } else {
        job
    };
    save_config_synchronously(job);
}

pub(crate) fn dispatch_config_save(
    event_tx: Sender<WorkerEvent>,
    path: PathBuf,
    config: Config,
    success: String,
    rollback: ConfigSaveRollback,
) {
    dispatch_config_save_job(
        config_save_tx(),
        Box::new(ConfigSaveJob {
            event_tx: event_tx.clone(),
            path,
            config,
            success,
            rollback,
        }),
    );
}

pub(crate) fn flush_config_saves() {
    let Some(config_save_tx) = config_save_tx() else {
        return;
    };
    let (done_tx, done_rx) = mpsc::channel();
    if config_save_tx.send(ConfigWorkerJob::Flush(done_tx)).is_ok() {
        let _ = done_rx.recv();
    }
}

#[cfg(test)]
enum ConfigSaveTestAction {
    Delay(Duration),
    Fail(String),
}

#[cfg(test)]
static CONFIG_SAVE_TEST_ACTIONS: std::sync::OnceLock<
    Mutex<HashMap<PathBuf, std::collections::VecDeque<ConfigSaveTestAction>>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
fn push_config_save_test_action(path: PathBuf, action: ConfigSaveTestAction) {
    CONFIG_SAVE_TEST_ACTIONS
        .get_or_init(Default::default)
        .lock()
        .expect("config-save test actions lock")
        .entry(path)
        .or_default()
        .push_back(action);
}

#[cfg(test)]
fn take_config_save_test_action(path: &Path) -> Option<ConfigSaveTestAction> {
    let mut actions = CONFIG_SAVE_TEST_ACTIONS
        .get_or_init(Default::default)
        .lock()
        .expect("config-save test actions lock");
    let action = actions.get_mut(path)?.pop_front();
    if actions.get(path).is_some_and(|queued| queued.is_empty()) {
        actions.remove(path);
    }
    action
}

#[cfg(test)]
pub(crate) fn delay_next_config_save(path: PathBuf, delay: Duration) {
    push_config_save_test_action(path, ConfigSaveTestAction::Delay(delay));
}

#[cfg(test)]
pub(crate) fn fail_next_config_save(path: PathBuf, message: impl Into<String>) {
    push_config_save_test_action(path, ConfigSaveTestAction::Fail(message.into()));
}

/// Run the synchronous git probes that gate "add project" — `is_git_repo`,
/// `current_branch`, and `remote_default_branch` — on a worker thread.
/// Results land as [`WorkerEvent::AddProjectMetaReady`] so the main loop
/// can either show the branch-mismatch warning, surface a non-repo error,
/// or proceed to `finish_add_project`.
pub(crate) fn dispatch_add_project_meta(
    tx: Sender<WorkerEvent>,
    path: PathBuf,
    name: String,
    workspace_mode: WorkspaceMode,
    paths: DuxPaths,
) {
    let _ = thread::Builder::new()
        .name(format!("add-project-meta-{}", path.display()))
        .spawn(move || {
            let result = if workspace_mode == WorkspaceMode::Shared
                && let Err(err) =
                    crate::config::validate_shared_workspace_path(&path.to_string_lossy(), &paths)
            {
                Err(crate::sanitize::for_terminal(&format!("{err:#}")))
            } else if !path.exists() {
                Err(format!("\"{}\" does not exist.", path.display()))
            } else if !git::is_git_repo(&path) {
                Err(format!("\"{}\" is not a git repository.", path.display()))
            } else if workspace_mode == WorkspaceMode::Shared {
                git::head_branch(&path)
                    .map(|branch| AddProjectMeta {
                        current_branch: branch.unwrap_or_else(|| DETACHED_HEAD_LABEL.to_string()),
                        remote_default: None,
                    })
                    .map_err(|err| format!("{err:#}"))
            } else {
                match git::current_branch(&path) {
                    Ok(branch) => {
                        let remote_default = git::remote_default_branch(&path);
                        Ok(AddProjectMeta {
                            current_branch: branch,
                            remote_default,
                        })
                    }
                    Err(err) => Err(format!("{err:#}")),
                }
            };
            let _ = tx.send(WorkerEvent::AddProjectMetaReady {
                path,
                name,
                workspace_mode,
                result,
            });
        });
}

pub(crate) fn dispatch_shared_reconnect_validation(
    tx: Sender<WorkerEvent>,
    session_id: String,
    workspace_path: String,
    paths: DuxPaths,
    force_fresh: bool,
) {
    let worker_tx = tx.clone();
    let worker_session_id = session_id.clone();
    let spawn = thread::Builder::new()
        .name("shared-reconnect-validation".to_string())
        .spawn(move || {
            let result = crate::config::validate_shared_workspace_path(&workspace_path, &paths)
                .map_err(|err| {
                    format!(
                        "Shared workspace is not eligible for reconnect: {}",
                        crate::sanitize::for_terminal(&format!("{err:#}"))
                    )
                })
                .and_then(|()| {
                    if Path::new(&workspace_path).exists() {
                        Ok(())
                    } else {
                        Err(format!(
                            "Shared workspace {} no longer exists. Restore it or delete and re-create the agent.",
                            crate::sanitize::for_terminal(&workspace_path)
                        ))
                    }
                });
            let _ = worker_tx.send(WorkerEvent::SharedReconnectValidated {
                session_id: worker_session_id,
                force_fresh,
                result,
            });
        });
    if let Err(err) = spawn {
        let _ = tx.send(WorkerEvent::SharedReconnectValidated {
            session_id,
            force_fresh,
            result: Err(format!(
                "Couldn't start shared-workspace reconnect validation: {}",
                crate::sanitize::for_terminal(&err.to_string())
            )),
        });
    }
}

pub(crate) fn browser_entries(dir: &Path) -> Result<Vec<BrowserEntry>, String> {
    let read = fs::read_dir(dir).map_err(|err| {
        format!(
            "Couldn't open directory {}: {err}",
            crate::sanitize::for_terminal(&dir.display().to_string())
        )
    })?;
    let mut entries = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let is_git_repo = path.join(".git").exists();
            let label = if is_git_repo {
                name
            } else {
                format!("{name}/")
            };
            Some(BrowserEntry {
                is_git_repo,
                path,
                label,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.is_git_repo
            .cmp(&a.is_git_repo)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    if let Some(parent) = dir.parent() {
        entries.insert(
            0,
            BrowserEntry {
                path: parent.to_path_buf(),
                label: "../".to_string(),
                is_git_repo: false,
            },
        );
    }
    Ok(entries)
}

// -- Disk + scrollback watchdog helpers --

/// How often [`App::spawn_disk_watchdog`] re-samples persistent-disk
/// usage. 60 s is short enough to react to a runaway worktree before the
/// host wedges, long enough that the syscall load is rounding-noise.
const DISK_WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);

/// How often [`App::spawn_scrollback_watchdog`] kicks the UI thread to
/// recompute total scrollback memory and consider an auto-detach.
const SCROLLBACK_WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);

/// Sample persistent-disk usage at `path` and return the percentage
/// (0..=100). `None` is returned on syscall failure (path missing,
/// permissions, etc.) — the watchdog skips that tick rather than emitting
/// a misleading "0% used" sample.
///
/// `statvfs` on Linux returns `f_blocks` (total fragments) and
/// `f_bavail` (fragments available to non-root); the byte total is
/// `f_frsize * f_blocks`. We compute the percentage with `(used * 100 /
/// total)` and clamp to `u8` so a fileystem that overflows an i64 (which
/// no realistic 2026-era host has) still produces a safe value.
pub(crate) fn sample_disk_usage_pct(path: &Path) -> Option<u8> {
    let stat = rustix::fs::statvfs(path).ok()?;
    let total_blocks = stat.f_blocks;
    if total_blocks == 0 {
        return None;
    }
    let avail_blocks = stat.f_bavail;
    let used_blocks = total_blocks.saturating_sub(avail_blocks);
    // Multiply before the divide to keep precision on small filesystems.
    let pct = (used_blocks.saturating_mul(100) / total_blocks).min(100);
    Some(pct as u8)
}

// -- GitHub PR sync helpers (run on background threads) --

#[derive(Default)]
struct PrSyncOutput {
    branch_updates: Vec<(String, String)>,
    pr_results: Vec<(String, Option<crate::model::PrInfo>)>,
}

fn run_pr_sync(sessions: &Arc<Mutex<Vec<PrSyncEntry>>>) -> PrSyncOutput {
    let snapshot = match sessions.lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return PrSyncOutput::default(),
    };
    run_pr_sync_snapshot_with(
        &snapshot,
        git::head_branch,
        check_pr_for_entry,
        check_pr_for_shared_entry,
    )
}

fn run_pr_sync_snapshot_with<H, R, S>(
    snapshot: &[PrSyncEntry],
    mut head_branch: H,
    mut check_regular: R,
    mut check_shared: S,
) -> PrSyncOutput
where
    H: FnMut(&Path) -> Result<Option<String>>,
    R: FnMut(&PrSyncEntry) -> Option<crate::model::PrInfo>,
    S: FnMut(&PrSyncEntry, &str) -> Option<crate::model::PrInfo>,
{
    let mut output = PrSyncOutput::default();
    let mut shared_groups: Vec<(PathBuf, Vec<&PrSyncEntry>)> = Vec::new();
    for entry in snapshot {
        if !entry.shared_workspace {
            output
                .pr_results
                .push((entry.session_id.clone(), check_regular(entry)));
            continue;
        }
        let path = PathBuf::from(&entry.worktree_path);
        let canonical = path.canonicalize().unwrap_or(path);
        if let Some((_, entries)) = shared_groups
            .iter_mut()
            .find(|(candidate, _)| *candidate == canonical)
        {
            entries.push(entry);
        } else {
            shared_groups.push((canonical, vec![entry]));
        }
    }

    for (path, entries) in shared_groups {
        let Ok(branch) = head_branch(&path) else {
            continue;
        };
        let (branch_label, pr) = match branch {
            Some(branch) => {
                let pr = check_shared(entries[0], &branch);
                (branch, pr)
            }
            None => (DETACHED_HEAD_LABEL.to_string(), None),
        };
        for entry in entries {
            if entry.branch_name != branch_label {
                output
                    .branch_updates
                    .push((entry.session_id.clone(), branch_label.clone()));
            }
            output
                .pr_results
                .push((entry.session_id.clone(), pr.clone()));
        }
    }
    output
}

fn check_pr_for_shared_entry(
    entry: &PrSyncEntry,
    live_branch: &str,
) -> Option<crate::model::PrInfo> {
    let owner_repo = git::remote_owner_repo(Path::new(&entry.worktree_path))?;
    discover_pr_by_branch(live_branch, &owner_repo, &entry.session_id)
}

/// Determine the current PR state for a session. The check strategy depends on
/// what we already know and whether the agent is still running:
///
/// | Known PR state | Agent running? | Action                                |
/// |----------------|---------------|---------------------------------------|
/// | None           | any           | `gh pr list --head` to discover       |
/// | OPEN           | any           | `gh pr view` + discover newer         |
/// | MERGED/CLOSED  | yes           | discover newer (agent may push again) |
/// | MERGED/CLOSED  | no            | **zero calls** — nothing will change  |
///
/// The last row is the key optimization: once a PR is in a terminal state and
/// the agent has exited, nobody is pushing to that branch anymore, so there is
/// no reason to check for newer PRs. This reduces API calls from O(sessions)
/// to O(active_sessions) for repos with many completed agents.
fn check_pr_for_entry(entry: &PrSyncEntry) -> Option<crate::model::PrInfo> {
    let owner_repo = git::remote_owner_repo(Path::new(&entry.worktree_path))
        .or_else(|| entry.known_pr.as_ref().map(|pr| pr.owner_repo.clone()))?;

    if let Some(ref known) = entry.known_pr {
        let is_terminal = known.state == "MERGED" || known.state == "CLOSED";

        if is_terminal {
            if entry.agent_exited {
                // Terminal PR + exited agent = zero network calls.
                // The agent process is gone and the PR is already merged/closed,
                // so no new commits or PRs will appear on this branch.
                return reconstruct_from_stored(known);
            }

            // Terminal PR but agent is still running — it might push new commits
            // and open a follow-up PR, so we still check for newer PRs.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &owner_repo, &entry.session_id)
                && newer.number > known.pr_number
            {
                return Some(newer);
            }
            return reconstruct_from_stored(known);
        }

        // Open PR: refresh its current state via `gh pr view`.
        if let Some(pr) = view_pr_by_number(known.pr_number, &known.owner_repo, &entry.session_id) {
            // Also check if a newer PR was opened.
            if let Some(newer) =
                discover_pr_by_branch(&entry.branch_name, &owner_repo, &entry.session_id)
                && newer.number > pr.number
            {
                return Some(newer);
            }
            return Some(pr);
        }
    }

    // No known PR — discover by branch name.
    discover_pr_by_branch(&entry.branch_name, &owner_repo, &entry.session_id)
}

/// Reconstruct a PrInfo from stored data without a network call.
/// Used for terminal states (merged/closed) that don't need refreshing.
fn reconstruct_from_stored(stored: &crate::storage::StoredPr) -> Option<crate::model::PrInfo> {
    use crate::model::{PrInfo, PrState};
    let state = match stored.state.as_str() {
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        "OPEN" => PrState::Open,
        _ => return None,
    };
    Some(PrInfo {
        number: stored.pr_number,
        state,
        title: stored.title.clone(),
        owner_repo: stored.owner_repo.clone(),
    })
}

/// Check a known PR by number using `gh pr view`.
fn view_pr_by_number(
    number: u64,
    owner_repo: &str,
    session_id: &str,
) -> Option<crate::model::PrInfo> {
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "view",
            &number.to_string(),
            "--repo",
            owner_repo,
            "--json",
            "number,state,title",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        logger::debug(&format!(
            "[gh-integration] gh pr view #{number} failed for {session_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_pr_json_object(text.trim(), owner_repo)
}

/// Discover a PR by branch name using `gh pr list --state all`.
fn discover_pr_by_branch(
    branch: &str,
    owner_repo: &str,
    session_id: &str,
) -> Option<crate::model::PrInfo> {
    let output = std::process::Command::new("gh")
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--repo",
            owner_repo,
            "--state",
            "all",
            "--json",
            "number,state,title",
            "--limit",
            "1",
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        logger::debug(&format!(
            "[gh-integration] gh pr list failed for {session_id}: {}",
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let arr: Vec<serde_json::Value> = serde_json::from_str(text.trim()).ok()?;
    let obj = arr.first()?;
    parse_pr_json_value(obj, owner_repo)
}

/// Parse a single PR JSON object (from `gh pr view` output).
fn parse_pr_json_object(json: &str, owner_repo: &str) -> Option<crate::model::PrInfo> {
    let obj: serde_json::Value = serde_json::from_str(json).ok()?;
    parse_pr_json_value(&obj, owner_repo)
}

/// Extract PrInfo from a serde_json::Value.
fn parse_pr_json_value(obj: &serde_json::Value, owner_repo: &str) -> Option<crate::model::PrInfo> {
    use crate::model::{PrInfo, PrState};

    let number = obj.get("number")?.as_u64()?;
    let state_str = obj.get("state")?.as_str()?;
    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state = match state_str {
        "OPEN" => PrState::Open,
        "MERGED" => PrState::Merged,
        "CLOSED" => PrState::Closed,
        _ => return None,
    };

    Some(PrInfo {
        number,
        state,
        title,
        owner_repo: owner_repo.to_string(),
    })
}

// ---- audit02 phase 14: SQLite WAL + integrity + periodic backup -------------
//
// `spawn_backup_worker` is added at the end of this file to minimize merge-
/// Spawn the periodic-backup worker.
///
/// The worker sleeps for `interval`, then asks `storage` to back itself up to
/// `<paths.root>/sessions.sqlite3.bak`. Errors are logged at warn level and do
/// not stop the loop — a transient I/O failure shouldn't take down the worker.
/// If `interval` is zero the function returns immediately without spawning.
pub fn spawn_backup_worker(
    storage: std::sync::Arc<crate::storage::SessionStore>,
    paths: crate::config::DuxPaths,
    interval: std::time::Duration,
    shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Option<std::thread::JoinHandle<()>> {
    if interval.is_zero() {
        crate::logger::info("[storage] periodic backup disabled (backup_interval_minutes = 0)");
        return None;
    }
    let dst = paths.root.join("sessions.sqlite3.bak");
    match std::thread::Builder::new()
        .name("storage-backup".into())
        .spawn(move || {
            while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(interval);
                if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                match storage.backup_to(&dst) {
                    Ok(()) => {
                        crate::logger::debug(&format!("[storage] backup ok -> {}", dst.display()))
                    }
                    Err(e) => crate::logger::warn(&format!(
                        "[storage] backup to {} failed: {e}",
                        dst.display()
                    )),
                }
            }
        }) {
        Ok(handle) => Some(handle),
        Err(e) => {
            crate::logger::warn(&format!("[storage] failed to spawn backup worker: {e}"));
            None
        }
    }
}

#[cfg(test)]
mod shared_workspace_create_tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn init_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["commit", "--allow-empty", "-m", "initial"]);
    }

    fn test_paths(root: PathBuf) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        }
    }

    fn project(path: &Path) -> Project {
        Project {
            id: "project".to_string(),
            name: "demo".to_string(),
            path: path.to_string_lossy().to_string(),
            default_provider: ProviderKind::from_str("codex"),
            current_branch: "main".to_string(),
            path_missing: false,
            meta_loaded: true,
        }
    }

    #[test]
    fn shared_create_subprocess_helper() {
        let Some(mode) = std::env::var_os("DUX_SHARED_CREATE_TEST_MODE") else {
            return;
        };
        let dux_home = PathBuf::from(std::env::var_os("DUX_SHARED_CREATE_HOME").unwrap());
        let repo = PathBuf::from(std::env::var_os("DUX_SHARED_CREATE_REPO").unwrap());
        let output = PathBuf::from(std::env::var_os("DUX_SHARED_CREATE_OUTPUT").unwrap());
        let paths = test_paths(dux_home);
        fs::create_dir_all(&paths.worktrees_root).unwrap();
        let mut config = Config::default();
        config.providers.commands["codex"].command = "dux-provider-does-not-exist".to_string();
        let provider = ProviderKind::from_str("codex");
        let request = if mode == "fork" {
            let now = Utc::now();
            CreateAgentRequest::ForkSession {
                project: project(&repo),
                source_session: Box::new(AgentSession {
                    id: "source".to_string(),
                    project_id: "project".to_string(),
                    project_path: Some(repo.to_string_lossy().to_string()),
                    provider: provider.clone(),
                    source_branch: "main".to_string(),
                    branch_name: "main".to_string(),
                    worktree_path: repo.to_string_lossy().to_string(),
                    agent_handle: "source".to_string(),
                    shared_workspace: true,
                    deleted_at: None,
                    title: Some("source".to_string()),
                    started_providers: Vec::new(),
                    provider_session_ids: Default::default(),
                    state: SessionState::Created { created_at: now },
                    settings: SessionSettings::default(),
                    created_at: now,
                    updated_at: now,
                }),
                source_label: "source".to_string(),
                custom_name: Some("fork-agent".to_string()),
                provider,
                settings: SessionSettings::default(),
            }
        } else {
            CreateAgentRequest::SharedWorkspace {
                project: project(&repo),
                agent_handle: Some("shared-agent".to_string()),
                provider,
                settings: SessionSettings::default(),
            }
        };
        let (tx, rx) = mpsc::channel();
        run_create_agent_job(
            request,
            paths,
            config,
            "store-test".to_string(),
            tx,
            (80, 24),
        );
        let outcome = rx
            .into_iter()
            .find_map(|event| match event {
                WorkerEvent::CreateAgentRecoverable { .. } => Some("recoverable"),
                WorkerEvent::CreateAgentFailed(_) => Some("failed"),
                WorkerEvent::CreateAgentReady(_) => Some("ready"),
                _ => None,
            })
            .unwrap_or("missing");
        fs::write(output, outcome).unwrap();
    }

    fn run_helper(mode: &str, root: &Path, repo: &Path) -> String {
        let output = root.join(format!("{mode}.out"));
        let status = Command::new(std::env::current_exe().unwrap())
            .arg("shared_create_subprocess_helper")
            .arg("--nocapture")
            .env("DUX_SHARED_CREATE_TEST_MODE", mode)
            .env("DUX_SHARED_CREATE_HOME", root.join("dux"))
            .env("DUX_SHARED_CREATE_REPO", repo)
            .env("DUX_SHARED_CREATE_OUTPUT", &output)
            .env("AMQ_GLOBAL_ROOT", root.join("amq"))
            .env_remove("AM_ROOT")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        fs::read_to_string(output).unwrap()
    }

    #[test]
    fn shared_create_uses_real_checkout_without_worktree_or_link() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("project");
        init_repo(&repo);
        let exclude = fs::read(repo.join(".git/info/exclude")).unwrap();

        assert_eq!(run_helper("shared", dir.path(), &repo), "recoverable");

        let paths = test_paths(dir.path().join("dux"));
        let rows = SessionStore::open(&paths.sessions_db_path)
            .unwrap()
            .load_sessions()
            .unwrap();
        assert_eq!(rows.len(), 1);
        let session = &rows[0];
        assert!(session.shared_workspace());
        assert_eq!(Path::new(&session.worktree_path), repo);
        assert_eq!(session.agent_handle(), "shared-agent");
        assert_eq!(session.title.as_deref(), Some("shared-agent"));
        assert!(session.state.is_retryable());
        assert_eq!(fs::read(repo.join(".git/info/exclude")).unwrap(), exclude);
        assert!(!repo.join(git::PROJECT_WORKTREES_LINK_NAME).exists());
        assert_eq!(fs::read_dir(&paths.worktrees_root).unwrap().count(), 0);
    }

    #[test]
    fn shared_create_rejects_managed_root_before_persisting() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("dux/worktrees/project");
        init_repo(&repo);

        assert_eq!(run_helper("managed", dir.path(), &repo), "failed");

        let paths = test_paths(dir.path().join("dux"));
        let rows = SessionStore::open(&paths.sessions_db_path)
            .unwrap()
            .load_sessions()
            .unwrap();
        assert!(rows.is_empty());
        assert!(!dir.path().join("amq/agents/shared-agent").exists());
    }

    #[test]
    fn shared_registration_accepts_detached_head_without_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("project");
        init_repo(&repo);
        let head = git::head_commit(&repo).unwrap();
        let status = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["checkout", "--detach", &head])
            .status()
            .unwrap();
        assert!(status.success());
        let (tx, rx) = mpsc::channel();

        dispatch_add_project_meta(
            tx,
            repo.clone(),
            "demo".to_string(),
            WorkspaceMode::Shared,
            test_paths(dir.path().join("dux")),
        );

        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let WorkerEvent::AddProjectMetaReady { result, .. } = event else {
            panic!("unexpected registration event");
        };
        let meta = result.expect("detached shared checkout is eligible");
        assert_eq!(meta.current_branch, DETACHED_HEAD_LABEL);
        assert!(meta.remote_default.is_none());
        assert_eq!(git::head_branch(&repo).unwrap(), None);
    }

    #[test]
    fn fork_from_shared_session_still_creates_isolated_worktree_row() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("project");
        init_repo(&repo);

        assert_eq!(run_helper("fork", dir.path(), &repo), "recoverable");

        let paths = test_paths(dir.path().join("dux"));
        let rows = SessionStore::open(&paths.sessions_db_path)
            .unwrap()
            .load_sessions()
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].shared_workspace());
        assert_ne!(Path::new(&rows[0].worktree_path), repo);
        assert!(
            Path::new(&rows[0].worktree_path)
                .canonicalize()
                .unwrap()
                .starts_with(paths.worktrees_root.canonicalize().unwrap())
        );
        assert!(Path::new(&rows[0].worktree_path).exists());
    }
}

#[cfg(test)]
mod shared_workspace_sync_tests {
    use super::*;
    use std::cell::Cell;

    fn branch_entry(id: &str, path: &Path, branch: &str, shared: bool) -> BranchSyncEntry {
        BranchSyncEntry {
            session_id: id.to_string(),
            worktree_path: path.to_string_lossy().to_string(),
            branch_name: branch.to_string(),
            shared_workspace: shared,
        }
    }

    fn pr_entry(id: &str, path: &Path, branch: &str, shared: bool) -> PrSyncEntry {
        PrSyncEntry {
            session_id: id.to_string(),
            branch_name: branch.to_string(),
            worktree_path: path.to_string_lossy().to_string(),
            shared_workspace: shared,
            known_pr: None,
            agent_exited: false,
        }
    }

    #[test]
    fn shared_branch_sync_queries_canonical_path_once_and_fans_out() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let alias = dir.path().join("shared-alias");
        std::os::unix::fs::symlink(&shared, &alias).unwrap();
        let snapshot = vec![
            branch_entry("shared-a", &shared, "old", true),
            branch_entry("shared-b", &alias, "old", true),
            branch_entry("worktree", &worktree, "old", false),
        ];
        let calls = Cell::new(0);

        let updates = App::collect_branch_sync_updates_with(&snapshot, |path| {
            calls.set(calls.get() + 1);
            Ok(Some(if path == worktree {
                "worktree-live".to_string()
            } else {
                "shared-live".to_string()
            }))
        });

        assert_eq!(
            calls.get(),
            2,
            "one shared path query plus one worktree query"
        );
        assert_eq!(
            updates,
            vec![
                ("worktree".to_string(), "worktree-live".to_string()),
                ("shared-a".to_string(), "shared-live".to_string()),
                ("shared-b".to_string(), "shared-live".to_string()),
            ]
        );
    }

    #[test]
    fn shared_pr_sync_discovers_once_per_path_and_ignores_known_session_shortcuts() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let mut snapshot = vec![
            pr_entry("shared-a", &shared, "stored-a", true),
            pr_entry("shared-b", &shared, "stored-b", true),
            pr_entry("worktree", &worktree, "stored-worktree", false),
        ];
        snapshot[0].known_pr = Some(crate::storage::StoredPr {
            session_id: "shared-a".to_string(),
            pr_number: 7,
            owner_repo: "old/repo".to_string(),
            state: "MERGED".to_string(),
            title: "stale shortcut".to_string(),
        });
        let head_calls = Cell::new(0);
        let regular_calls = Cell::new(0);
        let shared_calls = Cell::new(0);

        let output = run_pr_sync_snapshot_with(
            &snapshot,
            |_| {
                head_calls.set(head_calls.get() + 1);
                Ok(Some("live-shared".to_string()))
            },
            |_| {
                regular_calls.set(regular_calls.get() + 1);
                None
            },
            |_, branch| {
                shared_calls.set(shared_calls.get() + 1);
                assert_eq!(branch, "live-shared");
                Some(crate::model::PrInfo {
                    number: 42,
                    state: crate::model::PrState::Open,
                    title: "shared PR".to_string(),
                    owner_repo: "owner/repo".to_string(),
                })
            },
        );

        assert_eq!(head_calls.get(), 1);
        assert_eq!(shared_calls.get(), 1);
        assert_eq!(
            regular_calls.get(),
            1,
            "worktree behavior stays per-session"
        );
        assert_eq!(output.branch_updates.len(), 2);
        assert_eq!(
            output
                .pr_results
                .iter()
                .filter(|(id, pr)| id.starts_with("shared-") && pr.is_some())
                .count(),
            2
        );
    }

    #[test]
    fn detached_shared_head_fans_out_label_and_skips_pr_discovery() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = vec![
            pr_entry("shared-a", dir.path(), "main", true),
            pr_entry("shared-b", dir.path(), "main", true),
        ];
        let discoveries = Cell::new(0);

        let output = run_pr_sync_snapshot_with(
            &snapshot,
            |_| Ok(None),
            |_| unreachable!("no worktree entry"),
            |_, _| {
                discoveries.set(discoveries.get() + 1);
                None
            },
        );

        assert_eq!(discoveries.get(), 0);
        assert_eq!(
            output.branch_updates,
            vec![
                ("shared-a".to_string(), DETACHED_HEAD_LABEL.to_string()),
                ("shared-b".to_string(), DETACHED_HEAD_LABEL.to_string()),
            ]
        );
        assert!(output.pr_results.iter().all(|(_, pr)| pr.is_none()));
    }
}

#[cfg(test)]
mod config_save_divergence_tests {
    use super::*;
    use crate::config::Config;

    // audit03 P1-16 review S1: async config saves must not let a failed
    // earlier edit resurrect on disk when a later edit succeeds. The worker
    // patches only the field each edit changed (delta between the previous
    // requested snapshot and the desired one), so disk stays consistent with
    // the in-memory rollback.
    #[test]
    fn failed_edit_does_not_resurrect_when_later_edit_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        // Baseline on disk: theme=dark, left width 20.
        let mut base = Config::default();
        base.ui.theme = "dark".to_string();
        base.ui.left_width_pct = 20;
        let bindings = RuntimeBindings::from_keys_config(&base.keys);
        crate::config::save_config(&path, &base, &bindings).unwrap();

        // Edit A changes the theme, but its write fails. Nothing lands.
        let mut edit_a = base.clone();
        edit_a.ui.theme = "light".to_string();
        fail_next_config_save(path.clone(), "injected A failure");
        let res_a = save_config_change(&path, None, &edit_a);
        assert!(res_a.is_err(), "edit A was supposed to fail");

        // Edit B (queued after A, so its `previous` baseline is A's snapshot)
        // changes only the width. Its delta must touch width alone, leaving
        // the theme at the on-disk value — NOT resurrecting A's failed change.
        let mut edit_b = edit_a.clone();
        edit_b.ui.left_width_pct = 25;
        save_config_change(&path, Some(&edit_a), &edit_b).unwrap();

        let on_disk: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk.ui.left_width_pct, 25, "edit B width must persist");
        assert_eq!(
            on_disk.ui.theme, "dark",
            "failed edit A theme must not resurrect on disk"
        );
    }
}

#[cfg(test)]
mod section_advance_tests {
    use super::{GitFileAction, RightSection, section_after_git_file_op};

    #[test]
    fn staging_last_unstaged_file_advances_to_staged() {
        // Unstaged about to empty (1 → 0), staged has entries.
        assert_eq!(
            section_after_git_file_op(&GitFileAction::Stage, 2, 1),
            Some(RightSection::Staged)
        );
    }

    #[test]
    fn unstaging_last_staged_file_advances_to_unstaged() {
        assert_eq!(
            section_after_git_file_op(&GitFileAction::Unstage, 1, 3),
            Some(RightSection::Unstaged)
        );
    }

    #[test]
    fn no_advance_when_source_section_keeps_entries() {
        assert_eq!(section_after_git_file_op(&GitFileAction::Stage, 1, 2), None);
        assert_eq!(
            section_after_git_file_op(&GitFileAction::Unstage, 3, 1),
            None
        );
    }

    #[test]
    fn no_advance_when_destination_section_empty() {
        // Staging the last unstaged file but nothing is staged yet: staying
        // put is fine (the pane will show the now-empty list either way, and
        // there is no better section to move to).
        assert_eq!(section_after_git_file_op(&GitFileAction::Stage, 0, 1), None);
        assert_eq!(
            section_after_git_file_op(&GitFileAction::Unstage, 1, 0),
            None
        );
    }

    #[test]
    fn discard_never_advances() {
        assert_eq!(
            section_after_git_file_op(&GitFileAction::Discard { untracked: false }, 0, 1),
            None
        );
    }
}

#[cfg(test)]
mod backup_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn backup_worker_runs_on_schedule_and_zero_disables() {
        let temp = tempfile::tempdir().unwrap();
        let paths = crate::config::DuxPaths {
            root: temp.path().to_path_buf(),
            config_path: temp.path().join("config.toml"),
            sessions_db_path: temp.path().join("sessions.sqlite3"),
            worktrees_root: temp.path().join("worktrees"),
            lock_path: temp.path().join("dux.lock"),
        };
        let store = std::sync::Arc::new(
            crate::storage::SessionStore::open(&paths.sessions_db_path).unwrap(),
        );
        let disabled = spawn_backup_worker(
            std::sync::Arc::clone(&store),
            paths.clone(),
            std::time::Duration::ZERO,
            std::sync::Arc::new(AtomicBool::new(false)),
        );
        assert!(disabled.is_none());
        assert!(!paths.root.join("sessions.sqlite3.bak").exists());

        let shutdown = std::sync::Arc::new(AtomicBool::new(false));
        let handle = spawn_backup_worker(
            store,
            paths.clone(),
            std::time::Duration::from_millis(10),
            std::sync::Arc::clone(&shutdown),
        )
        .expect("worker");
        for _ in 0..100 {
            if paths.root.join("sessions.sqlite3.bak").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(paths.root.join("sessions.sqlite3.bak").exists());
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[test]
    fn app_run_wires_the_periodic_backup_worker() {
        let app_source = include_str!("mod.rs");
        let run_body = app_source
            .split_once("pub fn run(&mut self) -> Result<()> {")
            .and_then(|(_, tail)| tail.split_once("fn restore_sessions"))
            .map(|(run, _)| run)
            .expect("App::run source");
        assert!(run_body.contains("workers::spawn_backup_worker"));
        assert!(run_body.contains("backup_interval_minutes"));
    }
}
