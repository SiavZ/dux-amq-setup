//! Git/repo/session state grouped from the App god-object (audit02 P1-V).
//!
//! These fields cluster around: project + session lists, change-file caches
//! (staged/unstaged), the commit-message editor, and the in-flight markers
//! for git-driven background workers (commit, staged-diff, add-project,
//! deletions, changed-files dispatch debouncer). Worker callbacks and the
//! left/right pane render paths are the heaviest readers; input handling
//! mutates the lists and in-flight flags.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::model::{AgentSession, ChangedFile, Project};

use super::super::text_input::TextInput;

pub(crate) struct GitState {
    pub(crate) projects: Vec<Project>,
    pub(crate) sessions: Vec<AgentSession>,
    pub(crate) staged_files: Vec<ChangedFile>,
    pub(crate) unstaged_files: Vec<ChangedFile>,
    pub(crate) collapsed_projects: HashSet<String>,
    pub(crate) commit_input: TextInput,
    /// Tracks when each agent last received PTY data, for the streaming
    /// activity spinner in the left pane. Lives with `sessions` because it
    /// is keyed by session id and is read alongside the session list every
    /// frame.
    pub(crate) last_pty_activity: HashMap<String, Instant>,
    /// Last time `reload_changed_files` dispatched a one-shot
    /// `dispatch_changed_files` job. Used to debounce rapid session
    /// navigation — if the user paged through 10 sessions in a quarter
    /// second we'd otherwise spawn 10 git processes. The
    /// `spawn_changed_files_poller` covers steady-state refresh; this
    /// debounce just needs to suppress thundering herds on selection
    /// changes.
    pub(crate) last_changed_files_dispatch: Option<Instant>,
    /// Set to `true` while a one-shot `commit` worker is in flight so
    /// `execute_commit` can refuse re-entry. Cleared by
    /// `WorkerEvent::CommitFinished`.
    pub(crate) commit_in_flight: bool,
    /// Set to `true` while a one-shot `staged_diff` worker is in flight
    /// for the AI-commit-message generator. Cleared by
    /// `WorkerEvent::StagedDiffReady`.
    pub(crate) staged_diff_in_flight: bool,
    /// Set to `true` while an `add_project` git probe is in flight so
    /// duplicate kicks (e.g. user re-presses Enter on the path prompt)
    /// don't queue multiple workers. Cleared by
    /// `WorkerEvent::AddProjectMetaReady`.
    pub(crate) add_project_in_flight: bool,
    /// Session IDs spawned with resume args and the wall-clock time the resume
    /// attempt began. Used for one-shot fallbacks when resume exits quickly or
    /// hangs without rendering visible output.
    pub(crate) resume_fallback_candidates: HashMap<String, Instant>,
    /// Session IDs whose worktree is currently being removed by a background
    /// worker. Prevents duplicate delete requests from spawning a second
    /// worker while the first is still running; also drives the dimmed
    /// visual cue on the left pane row so the user can see the in-flight
    /// state.
    pub(crate) pending_deletions: HashSet<String>,
    /// Maps session IDs to the exact Busy message set by
    /// `begin_delete_session`. Used by the worker event handler to decide
    /// whether the current status-line content was set by this deletion (and
    /// should be cleared) or by an unrelated operation (and should be left
    /// alone). Cleared per-session when the worker event arrives.
    pub(crate) deletion_busy_messages: HashMap<String, String>,
}
