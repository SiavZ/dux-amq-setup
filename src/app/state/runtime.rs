//! Backend/concurrency state grouped from the App god-object (audit02 P1-V).
//!
//! These fields are all "runtime plumbing": worker channels, the PTY map, the
//! single-instance lockfile, OS-level atomics, and the GitHub/PR tracking
//! caches. They are accessed from worker callbacks and the main loop, but are
//! only incidentally touched by rendering code.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::amq_inject::QueuedMessage;
use crate::lockfile::SingleInstanceLock;
use crate::model::ProviderKind;

use super::super::{BranchSyncEntry, CompanionTerminal, PrSyncEntry, WorkerEvent};

pub(crate) struct RuntimeState {
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    // audit02 P1-Z phase 2 (Phase 18): the legacy `providers:
    // HashMap<String, PtyClient>` field is gone. PTY ownership now
    // lives inside `SessionState::Live` / `SessionState::Detached` on
    // each `AgentSession`. Look up handles via `App::find_pty_handle`.
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub(crate) running_provider_pins: HashMap<String, ProviderKind>,
    pub(crate) companion_terminals: HashMap<String, CompanionTerminal>,
    pub(crate) pulls_in_flight: HashSet<String>,
    pub(crate) watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub(crate) has_active_processes: Arc<AtomicBool>,
    pub(crate) sigwinch_flag: Arc<AtomicBool>,
    pub(crate) branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub(crate) gh_status: crate::model::GhStatus,
    pub(crate) github_integration_enabled: bool,
    pub(crate) pr_statuses: HashMap<String, crate::model::PrInfo>,
    pub(crate) pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    pub(crate) pr_sync_enabled: Arc<AtomicBool>,
    /// Timestamps of the last PR check per session, to avoid hammering on rapid
    /// state transitions.
    pub(crate) pr_last_checked: HashMap<String, Instant>,
    /// File-system watcher for `.git/refs/heads/` directories. `None` if the
    /// watcher could not be created (graceful fallback to poll-only).
    pub(crate) refs_watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Maps watched worktree paths back to session IDs so the refs watcher
    /// can route change events.
    pub(crate) refs_watch_paths: HashMap<PathBuf, String>,
    /// Exclusive lock held for the lifetime of this `App` so only one dux
    /// instance runs against a given config directory. Released
    /// automatically on drop (including crashes), so there is nothing to
    /// clean up on exit.
    pub(crate) _single_instance_lock: SingleInstanceLock,
    /// Per-session watch-rule engines. Attached when a session
    /// transitions into `SessionState::Live`, removed on exit. Sessions
    /// whose provider has no rules in the config never get an entry —
    /// the per-tick scan is then skipped entirely. See `crate::watch`.
    pub(crate) watch_engines: HashMap<String, crate::watch::WatchEngine>,
    /// Filesystem watcher for the AMQ inject-queue. Held to keep the
    /// inotify thread alive; `None` when the drainer is disabled or
    /// the watcher couldn't be created (graceful fallback to poll-only).
    /// See `crate::amq_inject` for the underlying machinery and
    /// `crate::app::amq_inject` for the App-side glue.
    pub(crate) amq_inject_watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Resolved queue root path. Cached so each tick doesn't re-resolve
    /// `XDG_DATA_HOME`. Empty when `[amq.inject].enabled = false` or
    /// resolution failed (no `$HOME`).
    pub(crate) amq_inject_queue_dir: Option<PathBuf>,
    /// Per-receiver delivery queue. Each entry is a `QueuedMessage`
    /// already claimed via the `.inflight.` rename, so only this App
    /// instance can deliver it. Drained by `App::tick_amq_inject` when
    /// the matching session is idle.
    pub(crate) amq_inject_pending: HashMap<String, VecDeque<QueuedMessage>>,
    /// Last time we surfaced a "no matching session for receiver X"
    /// status warning, keyed by receiver. Rate-limited so a queue full
    /// of messages for an unknown handle doesn't spam the status line.
    pub(crate) amq_inject_last_warned: HashMap<String, Instant>,
    /// Last time we emitted a `debug`-level "drainer holding for X"
    /// trace event, keyed by receiver. Independent of
    /// `amq_inject_last_warned` because the warning fires on
    /// permanently-stuck states (no session match, timeout) while
    /// the debug event also covers the transient busy-marker hold —
    /// useful for diagnosing why a delivery hasn't fired yet without
    /// being a user-visible error. Throttled to one event per
    /// receiver per `crate::app::inject_runtime::HOLD_LOG_RATE_LIMIT`
    /// (60 s) so the JSON log doesn't grow with the tick rate.
    pub(crate) amq_inject_last_held_logged: HashMap<String, Instant>,
}
