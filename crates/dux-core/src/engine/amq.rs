//! Engine-side AMQ runtime: session settings, the inject-queue drainer, and
//! the Orchestrator watchdog.
//!
//! Threading model (upstream's, not the fork's): the only background threads
//! are the `notify` watcher and one loop worker that posts
//! [`WorkerEvent::AmqInjectScanRequested`] on the ordinary worker lane. Every
//! claim, PTY write and unlink happens on the engine's own thread, driven by
//! [`Engine::tick_amq`], which BOTH surfaces call once per loop iteration.
//! So nothing off-thread ever holds a queue claim, and a hot reload that
//! execs the process leaves only `.inflight.*.msg` files the next process's
//! startup reclaim puts back (0e8efd57).
//!
//! Seams to other workstreams are marked `INTEGRATION:`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::amq::delivery::{self, DeliveryPhase, HoldReason, ReceiverCandidate};
use crate::amq::orchestrator::{self as orch, OrchestratorPeer};
use crate::amq::queue::{self, QueuedMessage, UNROUTED_RECEIVER};
use crate::engine::{Engine, EventReaction, LoopControl, LoopWorkerSpec, StatusUpdate};
use crate::model::{AgentSession, SessionStatus};
use crate::session_settings::{ContextMode, ORCHESTRATOR_SYSTEM_PROMPT, SessionSettings};
use crate::worker::WorkerEvent;

/// Repeat-warning throttle for an undeliverable receiver.
const WARN_RATE_LIMIT: Duration = Duration::from_secs(60);
/// Throttle for the per-receiver "holding" debug line.
const HOLD_LOG_RATE_LIMIT: Duration = Duration::from_secs(60);
/// Files beyond this per-scan budget stay as `.msg` for later scans.
const MAX_INJECT_CLAIMS_PER_SCAN: usize = 32;
/// Cap on claimed-but-undelivered messages (bounds crash recovery work).
const MAX_INJECT_PENDING_TOTAL: usize = 128;
/// Per-receiver cap so one noisy handle cannot starve others.
const MAX_INJECT_PENDING_PER_RECEIVER: usize = 32;
/// Bound PTY writes per tick so a backlog cannot monopolise the loop.
const MAX_INJECT_ACTIONS_PER_TICK: usize = 16;
/// How long after a delivery the watch engine should skip the target, so the
/// `[task-done]` token in the Worker postscript cannot false-fire auto-clear.
pub const WATCH_SUPPRESS_AFTER_INJECT: Duration = Duration::from_secs(10);

/// All AMQ runtime state the engine owns. `Default` is the idle state; the
/// watcher is only started by [`Engine::start_amq`].
#[derive(Default)]
pub struct AmqRuntime {
    /// Per-session settings, loaded from `agent_sessions.session_settings`
    /// at construction and restore. Absent means `SessionSettings::default()`.
    pub session_settings: HashMap<String, SessionSettings>,
    pub queue_dir: Option<PathBuf>,
    pub watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Set on [`Engine::stop_amq`]; the poll loop exits on it (3d520748).
    pub shutdown: Arc<AtomicBool>,
    pub started: bool,
    pub pending: HashMap<String, VecDeque<QueuedMessage>>,
    pub startup_grace_until: Option<Instant>,
    pub cooldown_until: HashMap<String, Instant>,
    pub last_warned: HashMap<String, Instant>,
    pub first_pending_at: HashMap<String, Instant>,
    pub timeout_warned: HashSet<String>,
    pub last_held_logged: HashMap<String, Instant>,
    /// Last operator keystroke per SESSION. Upstream's `pty_input` keeps
    /// only a 1.25 s typing window and is cleared with the tab, so it cannot
    /// answer "has the user typed in the last 60 s"; this map can. Stamped
    /// by [`Engine::note_amq_user_input`], never by programmatic writes.
    pub last_user_keystroke: HashMap<String, Instant>,
    /// Sessions whose watch engine must not observe until the instant, so
    /// the Worker postscript cannot false-fire auto-clear.
    ///
    /// INTEGRATION: herb's watch tick must skip a session while
    /// `Engine::amq_watch_suppressed(id)` is true and rebaseline once it
    /// expires (fork `tick_watch_engines`).
    pub watch_suppress_until: HashMap<String, Instant>,
    /// Orchestrator watchdog: first-seen / last nudge per session.
    pub orchestrator_last_nudged: HashMap<String, Instant>,
    /// Last checkpoint per project, so several orchestrators in one project
    /// do not all poll the same workers (cc7005d0).
    pub orchestrator_project_last_checkpoint: HashMap<String, Instant>,
    /// Orchestrators that already received their typed startup policy.
    pub orchestrator_policy_injected: HashSet<String>,
    /// Watchdog prompts whose body was typed and whose Enter is pending.
    ///
    /// INTEGRATION: the fork shared this map with watch-rule SendText
    /// (`watch_pending_enters`); herb's watch port may merge the two.
    pub pending_enters: HashMap<String, Instant>,
}

/// A surface's view of which agent the operator is typing into right now.
/// The TUI passes its focused session when input goes to the agent; the web
/// passes `None` (nobody is focused in the fork's sense), so only the
/// keystroke timestamps gate delivery there.
#[derive(Clone, Copy, Debug, Default)]
pub struct AmqFocus<'a> {
    pub focused_session: Option<&'a str>,
    /// The session the unrouted fallback delivers to.
    pub selected_session: Option<&'a str>,
}

impl Engine {
    // ─── session settings ────────────────────────────────────────────

    /// The saved settings for `session_id`. `None` means defaults.
    ///
    /// INTEGRATION: herb's watch engine reads `watch_rule_arm` and
    /// `auto_clear_on_task_done` through this getter.
    pub fn session_settings(&self, session_id: &str) -> Option<&SessionSettings> {
        self.amq.session_settings.get(session_id)
    }

    /// Settings for `session_id`, defaulting when none are saved.
    pub fn session_settings_or_default(&self, session_id: &str) -> SessionSettings {
        self.session_settings(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Context mode for `session_id`.
    pub fn session_mode(&self, session_id: &str) -> ContextMode {
        self.session_settings(session_id)
            .map(|s| s.mode)
            .unwrap_or_default()
    }

    /// Re-read every session's settings from SQLite. Called at engine
    /// construction and restore on both surfaces, so a hot reload (a fresh
    /// process) keeps every agent's mode and YOLO.
    pub fn load_session_settings_from_store(&mut self) {
        match self.session_store.load_session_settings() {
            Ok(map) => self.amq.session_settings = map,
            Err(err) => crate::logger::warn(&format!(
                "session settings could not be loaded; every agent reads as default: {err}"
            )),
        }
    }

    /// Persist-then-apply (fork 773a6b04 / audit03 P1-16): the store write
    /// happens against a candidate first, so a failure leaves memory exactly
    /// as it was and the caller can retry.
    pub(crate) fn set_session_settings(
        &mut self,
        session_id: &str,
        settings: SessionSettings,
        title: Option<Option<String>>,
    ) -> anyhow::Result<EventReaction> {
        let Some(index) = self.sessions.iter().position(|s| s.id == session_id) else {
            return Ok(EventReaction::Status(StatusUpdate::error(
                "Session disappeared while editing settings.",
            )));
        };
        let settings = normalize_settings(settings);
        let previous = self.session_settings_or_default(session_id);
        let new_title = title.map(|t| t.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        let title_changed = new_title
            .as_ref()
            .is_some_and(|t| *t != self.sessions[index].title);

        if title_changed {
            let mut candidate = self.sessions[index].clone();
            candidate.title = new_title.clone().flatten();
            candidate.updated_at = chrono::Utc::now();
            self.session_store.upsert_session(&candidate)?;
        }
        self.session_store
            .set_session_settings(session_id, &settings)?;

        // Both writes landed: now mutate live memory.
        if title_changed {
            let session = &mut self.sessions[index];
            session.title = new_title.flatten();
            session.updated_at = chrono::Utc::now();
        }
        if settings.is_default() {
            self.amq.session_settings.remove(session_id);
        } else {
            self.amq.session_settings.insert(session_id.to_string(), settings.clone());
        }
        // A mode change re-arms the watchdog's startup-policy bookkeeping.
        if previous.mode != settings.mode {
            self.amq.orchestrator_policy_injected.remove(session_id);
            self.amq.orchestrator_last_nudged.remove(session_id);
        }
        // INTEGRATION: herb's watch engine must re-apply `watch_rule_arm`
        // and rebuild the built-in auto-clear rule when mode/auto-clear
        // change (fork `apply_session_settings_to_runtime`).

        let system_prompt_changed = previous.system_prompt != settings.system_prompt;
        let needs_respawn = previous.yolo_permissions != settings.yolo_permissions
            || previous.verify_envelope_override != settings.verify_envelope_override
            || system_prompt_changed;
        let summary = build_session_settings_save_summary(
            previous.mode != settings.mode,
            title_changed,
            previous.auto_clear_on_task_done != settings.auto_clear_on_task_done,
            system_prompt_changed,
            needs_respawn,
        );
        Ok(EventReaction::Multi(vec![
            EventReaction::RebuildLeftItems,
            EventReaction::Status(if needs_respawn {
                StatusUpdate::warning(format!(
                    "{summary} Reconnect the agent for spawn-time settings (YOLO, AMQ verify, system prompt) to take effect."
                ))
            } else {
                StatusUpdate::info(summary)
            }),
        ]))
    }

    // ─── user input ──────────────────────────────────────────────────

    /// Drop every per-session AMQ entry for a deleted agent. The DB row (and
    /// its settings column) is removed by the delete itself.
    pub(crate) fn forget_amq_session(&mut self, session_id: &str) {
        let a = &mut self.amq;
        a.session_settings.remove(session_id);
        a.last_user_keystroke.remove(session_id);
        a.watch_suppress_until.remove(session_id);
        a.cooldown_until.remove(session_id);
        a.pending_enters.remove(session_id);
        a.orchestrator_last_nudged.remove(session_id);
        a.orchestrator_policy_injected.remove(session_id);
    }

    /// Record that the operator typed into `session_id`. Surfaces call this
    /// next to `note_pty_input` for agent tabs; programmatic writes (AMQ,
    /// watch effects, macros sent by rules) must NOT, or the quiet window
    /// feeds back into itself.
    pub fn note_amq_user_input(&mut self, session_id: &str) {
        self.amq
            .last_user_keystroke
            .insert(session_id.to_string(), Instant::now());
    }

    /// The session owning a tab id, for surfaces that only hold the tab.
    pub fn session_id_for_tab(&self, tab_id: &str) -> Option<String> {
        if let Some(s) = self.sessions.iter().find(|s| s.slot_tab_id().as_str() == tab_id) {
            return Some(s.id.clone());
        }
        self.agent_tabs
            .get(crate::ids::TabIdRef::new(tab_id))
            .map(|t| t.session_id.clone())
    }

    /// Whether the watch engine should skip `session_id` right now.
    ///
    /// INTEGRATION: herb's watch tick consults this.
    pub fn amq_watch_suppressed(&self, session_id: &str) -> bool {
        self.amq
            .watch_suppress_until
            .get(session_id)
            .is_some_and(|until| Instant::now() < *until)
    }

    /// Whether AMQ work for this session makes Worker auto-clear unsafe:
    /// a busy footer, a claimed or queued wake, unread/pending mail, or
    /// recent collaboration (b6638ae0, 22e6b1e3).
    ///
    /// INTEGRATION: herb's auto-clear rule must rebaseline instead of firing
    /// while this is true (fork `should_suppress_auto_clear`).
    pub fn amq_blocks_auto_clear(&self, session_id: &str) -> bool {
        let Some(session) = self.session_by_id(session_id) else {
            return true;
        };
        let settings = self.session_settings_or_default(session_id);
        if settings.mode != ContextMode::Worker || !settings.auto_clear_on_task_done {
            return true;
        }
        let cfg = &self.config.amq.inject;
        if let Some(client) = self.providers.get(session.slot_tab_id())
            && queue::snapshot_busy_marker(
                &client.scan_recent_lines(cfg.busy_scan_lines.max(1)),
                &cfg.busy_markers,
            )
            .is_some()
        {
            return true;
        }
        let receiver = amq_receiver_for_session(session);
        if self.amq.pending.get(&receiver).is_some_and(|q| !q.is_empty()) {
            return true;
        }
        if let Some(dir) = &self.amq.queue_dir
            && crate::amq::activity::has_pending_inject(dir, &receiver)
        {
            return true;
        }
        let agent_dir = amq_root_for_collaboration_guard()
            .join("agents")
            .join(&receiver);
        if crate::amq::activity::has_pending_mail(&agent_dir) {
            return true;
        }
        crate::amq::activity::has_recent_activity(
            &agent_dir,
            Duration::from_secs(cfg.auto_clear_collaboration_quiet_secs),
            SystemTime::now(),
        )
    }

    // ─── lifecycle ───────────────────────────────────────────────────

    /// Start the inject-queue watcher. Idempotent (the flip hands a live
    /// engine to the other surface, which calls this again). Never fatal:
    /// dux must come up when `$HOME` is unwritable or notify fails.
    pub fn start_amq(&mut self) {
        if self.amq.started {
            return;
        }
        let cfg = self.config.amq.inject.clone();
        if !cfg.enabled {
            crate::logger::info("amq: inject drainer disabled in config");
            return;
        }
        let Some(queue_dir) = queue::resolve_queue_dir(&cfg) else {
            crate::logger::warn("amq: could not resolve inject queue dir (no $HOME?)");
            return;
        };
        self.amq.started = true;
        self.amq.queue_dir = Some(queue_dir.clone());
        let max_age = max_message_age(cfg.max_message_age_secs);
        match queue::reclaim_stale_inflight_with_max_age(&queue_dir, max_age) {
            Ok(o) if o.reclaimed == 0 && o.expired == 0 => {}
            Ok(o) => crate::logger::info(&format!(
                "amq: processed stale inflight files from a prior dux (reclaimed={} expired={})",
                o.reclaimed, o.expired
            )),
            Err(err) => crate::logger::warn(&format!("amq: reclaim sweep failed: {err}")),
        }
        match queue::expire_stale_messages(&queue_dir, max_age) {
            Ok(0) => {}
            Ok(n) => crate::logger::warn(&format!(
                "amq: expired {n} stale queued messages before the startup scan"
            )),
            Err(err) => crate::logger::warn(&format!("amq: stale message expiry failed: {err}")),
        }
        match queue::spawn_inject_watcher(&queue_dir, self.worker_tx.clone(), || {
            WorkerEvent::AmqInjectScanRequested
        }) {
            Ok(watcher) => self.amq.watcher = Some(watcher),
            Err(err) => crate::logger::warn(&format!(
                "amq: notify watcher failed; relying on the polling fallback: {err}"
            )),
        }
        let shutdown = Arc::new(AtomicBool::new(false));
        self.amq.shutdown = Arc::clone(&shutdown);
        let interval = Duration::from_millis(cfg.poll_interval_ms.max(100));
        self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "amq-inject-poll".into(),
                feature: "AMQ message delivery".into(),
                remedy: crate::poller_status::REMEDY_RESTART_DUX.into(),
            },
            move |tx| {
                // Sleep in short slices so `stop_amq` is honoured promptly.
                let mut slept = Duration::ZERO;
                while slept < interval {
                    if shutdown.load(Ordering::Relaxed) {
                        return LoopControl::Break;
                    }
                    let slice = (interval - slept).min(Duration::from_millis(50));
                    std::thread::sleep(slice);
                    slept += slice;
                }
                if shutdown.load(Ordering::Relaxed)
                    || tx.send(WorkerEvent::AmqInjectScanRequested).is_err()
                {
                    return LoopControl::Break;
                }
                LoopControl::Continue
            },
        );
        self.amq.startup_grace_until =
            Some(Instant::now() + Duration::from_millis(cfg.startup_grace_ms));
        // First scan now, so messages queued while dux was down are claimed.
        let _ = self.worker_tx.send(WorkerEvent::AmqInjectScanRequested);
        crate::logger::info(&format!(
            "amq: inject drainer initialised at {}",
            queue_dir.display()
        ));
    }

    /// Stop the watcher and poll thread. Claimed-but-undelivered messages
    /// stay `.inflight.*.msg` on disk; the next process reclaims them.
    pub fn stop_amq(&mut self) {
        self.amq.shutdown.store(true, Ordering::Relaxed);
        self.amq.watcher = None;
        self.amq.started = false;
    }

    // ─── scan (claim) ────────────────────────────────────────────────

    /// Claim new `.msg` files onto the in-memory pending queue, under the
    /// per-scan, per-receiver and total budgets, round-robin across receivers
    /// (867b4357).
    pub(crate) fn drain_amq_inject_queue(&mut self) -> EventReaction {
        let Some(queue_dir) = self.amq.queue_dir.clone() else {
            return EventReaction::Nothing;
        };
        let cfg = &self.config.amq.inject;
        let max_bytes = cfg.max_message_bytes;
        let max_age = max_message_age(cfg.max_message_age_secs);
        let now_wall = SystemTime::now();
        let mut total_pending: usize = self.amq.pending.values().map(VecDeque::len).sum();
        let scan_limit =
            MAX_INJECT_CLAIMS_PER_SCAN.min(MAX_INJECT_PENDING_TOTAL.saturating_sub(total_pending));
        if scan_limit == 0 {
            return EventReaction::Nothing;
        }
        let outcome = match queue::scan_queue_dir_limited(&queue_dir, scan_limit) {
            Ok(o) => o,
            Err(err) => {
                crate::logger::warn(&format!("amq: scan failed: {err}"));
                return EventReaction::Nothing;
            }
        };
        for (path, reason) in &outcome.rejections {
            crate::logger::warn(&format!(
                "amq: queue entry rejected ({}): {}",
                path.display(),
                reason.human()
            ));
        }
        let mut statuses = Vec::new();
        let mut claimed = 0usize;
        let mut deferred = 0usize;
        for pending in outcome.messages {
            if queue::is_file_older_than(&pending.path, max_age, now_wall) {
                match queue::quarantine_expired(&pending.path) {
                    Ok(to) => crate::logger::warn(&format!(
                        "amq: expired stale message before claim ({})",
                        to.display()
                    )),
                    Err(err) => crate::logger::warn(&format!("amq: expiry failed: {err}")),
                }
                continue;
            }
            let depth = self.amq.pending.get(&pending.receiver).map_or(0, VecDeque::len);
            if claimed >= MAX_INJECT_CLAIMS_PER_SCAN
                || total_pending >= MAX_INJECT_PENDING_TOTAL
                || depth >= MAX_INJECT_PENDING_PER_RECEIVER
            {
                deferred += 1;
                continue;
            }
            let modified_at = queue::modified_at(&pending.path);
            let inflight = match queue::claim(&pending.path) {
                Ok(p) => p,
                Err(err) => {
                    crate::logger::debug(&format!("amq: claim failed, retrying later: {err}"));
                    continue;
                }
            };
            claimed += 1;
            match queue::read_validated(&inflight, max_bytes) {
                Ok(body) => {
                    self.amq
                        .pending
                        .entry(pending.receiver.clone())
                        .or_default()
                        .push_back(QueuedMessage {
                            receiver: pending.receiver.clone(),
                            body,
                            inflight_path: inflight,
                            source_path: pending.path.clone(),
                            modified_at,
                            body_typed: false,
                            body_typed_at: None,
                        });
                    total_pending += 1;
                }
                Err(rejection) => {
                    // Quarantine so startup recovery does not reclaim and
                    // re-reject the same payload forever (b0c52c34).
                    let where_to = queue::quarantine_rejected(&inflight)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|e| format!("quarantine failed: {e}"));
                    crate::logger::warn(&format!(
                        "amq: rejected after claim, quarantined to {where_to}: {}",
                        rejection.human()
                    ));
                    statuses.push(EventReaction::Status(StatusUpdate::warning(format!(
                        "AMQ inject: {}",
                        rejection.human()
                    ))));
                }
            }
        }
        if deferred > 0 {
            crate::logger::info(&format!(
                "amq: deferred {deferred} files at the drainer's safety budget (claimed {claimed})"
            ));
        }
        reaction_from(statuses)
    }

    // ─── tick (deliver) ──────────────────────────────────────────────

    /// One drainer + watchdog pass. Both surfaces call this once per loop
    /// iteration and route the returned reaction like any other.
    pub fn tick_amq(&mut self, focus: AmqFocus<'_>) -> EventReaction {
        let mut out = Vec::new();
        self.tick_amq_inject(focus, &mut out);
        self.flush_amq_pending_enters();
        self.tick_orchestrator_watchdog(focus);
        reaction_from(out)
    }

    fn tick_amq_inject(&mut self, focus: AmqFocus<'_>, out: &mut Vec<EventReaction>) {
        if !self.config.amq.inject.enabled || self.amq.pending.is_empty() {
            return;
        }
        let now = Instant::now();
        if let Some(until) = self.amq.startup_grace_until {
            if now < until {
                return;
            }
            self.amq.startup_grace_until = None;
        }
        let cfg = self.config.amq.inject.clone();
        let timeout = Duration::from_secs(cfg.delivery_timeout_secs);
        let phase_delay = delivery::effective_enter_phase_delay(cfg.phase_delay_ms);
        let receivers: Vec<String> = self.amq.pending.keys().cloned().collect();
        let mut actions = 0usize;
        for receiver in receivers {
            if actions >= MAX_INJECT_ACTIONS_PER_TICK {
                break;
            }
            let session_id = if receiver == UNROUTED_RECEIVER {
                focus.selected_session.map(str::to_string)
            } else {
                self.find_session_for_receiver(&receiver)
            };
            let Some(session_id) = session_id else {
                self.warn_no_session(&receiver, now, out);
                self.warn_timeout(&receiver, now, timeout, out);
                self.log_holding(&receiver, now, HoldReason::NoSession);
                continue;
            };
            if self.amq.pending.get(&receiver).is_none_or(VecDeque::is_empty) {
                continue;
            }
            if self.expire_pending_head_if_stale(&receiver, out) {
                actions += 1;
                continue;
            }
            let phase = self.amq_delivery_phase(&receiver);

            // Submit-pending is recovery, not preflight: the body is already
            // in the prompt, so only Enter may follow (e9b888c8).
            if let Some(DeliveryPhase::SubmitPending { typed_at }) = phase {
                if typed_at.is_some_and(|at| now.duration_since(at) < phase_delay) {
                    continue;
                }
                let Some(client) = self.slot_client(&session_id) else {
                    self.warn_timeout(&receiver, now, timeout, out);
                    self.log_holding(&receiver, now, HoldReason::PtyGone);
                    continue;
                };
                let submit = delivery::submit_key_bytes_for_provider(
                    self.running_provider_of(&session_id).as_ref(),
                );
                actions += 1;
                match client.write_bytes(submit) {
                    Ok(()) => {
                        let msg = self
                            .amq
                            .pending
                            .get_mut(&receiver)
                            .and_then(VecDeque::pop_front)
                            .expect("head exists per phase peek");
                        self.on_amq_delivered(&session_id, &receiver, &msg, out);
                    }
                    Err(err) => {
                        crate::logger::warn(&format!(
                            "amq: Enter write failed for {receiver}; retrying Enter only: {err}"
                        ));
                        out.push(EventReaction::Status(StatusUpdate::warning(format!(
                            "AMQ inject: Enter submit failed for {receiver}; will retry without retyping."
                        ))));
                    }
                }
                self.remove_empty_inject_queue(&receiver);
                continue;
            }

            // Pre-delivery: nothing typed yet, holds are safe.
            if focus.focused_session == Some(session_id.as_str()) {
                let quiet = Duration::from_secs(cfg.active_session_quiet_secs);
                let last = self.amq.last_user_keystroke.get(&session_id).copied();
                if delivery::should_hold_for_quiet_window(last, now, quiet) {
                    self.log_holding(&receiver, now, HoldReason::UserTyping);
                    continue;
                }
            }
            if let Some(until) = self.amq.cooldown_until.get(&session_id).copied() {
                if delivery::should_hold_for_post_delivery_cooldown(phase, Some(until), now) {
                    self.log_holding(&receiver, now, HoldReason::PostDeliveryCooldown);
                    continue;
                }
                self.amq.cooldown_until.remove(&session_id);
            }
            let Some(client) = self.slot_client(&session_id) else {
                self.warn_timeout(&receiver, now, timeout, out);
                self.log_holding(&receiver, now, HoldReason::PtyGone);
                continue;
            };
            let snapshot = client.scan_recent_lines(cfg.busy_scan_lines.max(1));
            if let Some(marker) = queue::snapshot_busy_marker(&snapshot, &cfg.busy_markers) {
                let marker = marker.to_string();
                self.warn_timeout(&receiver, now, timeout, out);
                self.log_holding(&receiver, now, HoldReason::BusyMarker(&marker));
                continue;
            }
            // Phase 1: type the body (plus the Worker postscript).
            let body = self.amq.pending[&receiver]
                .front()
                .expect("non-empty")
                .body
                .clone();
            let with_postscript =
                delivery::apply_inject_postscript(&body, self.session_mode(&session_id));
            let payload = delivery::inject_body_bytes_for_provider(
                &with_postscript,
                self.running_provider_of(&session_id).as_ref(),
            );
            actions += 1;
            match self.slot_client(&session_id).map(|c| c.write_bytes(&payload)) {
                Some(Ok(())) => {
                    if let Some(head) = self.amq.pending.get_mut(&receiver).and_then(|q| q.front_mut()) {
                        head.body_typed = true;
                        head.body_typed_at = Some(now);
                    }
                    self.amq
                        .watch_suppress_until
                        .insert(session_id.clone(), now + WATCH_SUPPRESS_AFTER_INJECT);
                    crate::logger::debug(&format!(
                        "amq: typed wake body for {receiver} (phase 1): {}",
                        queue::preview(&body, 80)
                    ));
                }
                Some(Err(err)) => crate::logger::warn(&format!(
                    "amq: phase-1 write failed for {receiver}; retrying next tick: {err}"
                )),
                None => {}
            }
        }
    }

    fn on_amq_delivered(
        &mut self,
        session_id: &str,
        receiver: &str,
        msg: &QueuedMessage,
        out: &mut Vec<EventReaction>,
    ) {
        if let Err(err) = std::fs::remove_file(&msg.inflight_path) {
            crate::logger::warn(&format!(
                "amq: delivered but unlink of {} failed; may be re-delivered next start: {err}",
                msg.inflight_path.display()
            ));
        }
        crate::logger::info(&format!(
            "amq: delivered wake to {receiver}: {}",
            queue::preview(&msg.body, 80)
        ));
        out.push(EventReaction::Status(StatusUpdate::info(format!(
            "Delivered AMQ wake to {receiver}: {}",
            queue::preview(&msg.body, 60)
        ))));
        let now = Instant::now();
        self.amq
            .watch_suppress_until
            .insert(session_id.to_string(), now + WATCH_SUPPRESS_AFTER_INJECT);
        let cooldown = Duration::from_millis(self.config.amq.inject.post_delivery_cooldown_ms);
        if !cooldown.is_zero() {
            self.amq
                .cooldown_until
                .insert(session_id.to_string(), now + cooldown);
        }
    }

    fn amq_delivery_phase(&self, receiver: &str) -> Option<DeliveryPhase> {
        self.amq
            .pending
            .get(receiver)
            .and_then(VecDeque::front)
            .map(|head| {
                if head.body_typed {
                    DeliveryPhase::SubmitPending {
                        typed_at: head.body_typed_at,
                    }
                } else {
                    DeliveryPhase::TypeBody
                }
            })
    }

    fn remove_empty_inject_queue(&mut self, receiver: &str) {
        if self.amq.pending.get(receiver).is_some_and(VecDeque::is_empty) {
            self.amq.pending.remove(receiver);
            self.amq.last_warned.remove(receiver);
            self.amq.first_pending_at.remove(receiver);
            self.amq.timeout_warned.remove(receiver);
            self.amq.last_held_logged.remove(receiver);
        }
    }

    /// Drop one stale HELD wake. Only wakes not yet typed expire; a typed
    /// one still gets its Enter so no prompt is left floating (edd6a644).
    fn expire_pending_head_if_stale(&mut self, receiver: &str, out: &mut Vec<EventReaction>) -> bool {
        let max_age_secs = self.config.amq.inject.max_message_age_secs;
        if max_age_secs == 0 {
            return false;
        }
        let Some(head) = self.amq.pending.get(receiver).and_then(VecDeque::front) else {
            return false;
        };
        if head.body_typed {
            return false;
        }
        let Some(age) = head
            .modified_at
            .and_then(|m| SystemTime::now().duration_since(m).ok())
        else {
            return false;
        };
        if age <= Duration::from_secs(max_age_secs) {
            return false;
        }
        let msg = self
            .amq
            .pending
            .get_mut(receiver)
            .and_then(VecDeque::pop_front)
            .expect("head exists");
        match queue::quarantine_expired(&msg.inflight_path) {
            Ok(to) => {
                crate::logger::warn(&format!(
                    "amq: expired held wake for {receiver} after {}s ({})",
                    age.as_secs(),
                    to.display()
                ));
                out.push(EventReaction::Status(StatusUpdate::warning(format!(
                    "AMQ inject: expired stale message for \"{receiver}\" after {}s; inspect .expired if needed.",
                    age.as_secs()
                ))));
            }
            Err(err) => crate::logger::warn(&format!(
                "amq: failed to expire held wake for {receiver}; dropping it from memory: {err}"
            )),
        }
        self.remove_empty_inject_queue(receiver);
        true
    }

    fn log_holding(&mut self, receiver: &str, now: Instant, reason: HoldReason<'_>) {
        let due = self
            .amq
            .last_held_logged
            .get(receiver)
            .is_none_or(|t| now.duration_since(*t) >= HOLD_LOG_RATE_LIMIT);
        if !due {
            return;
        }
        self.amq
            .last_held_logged
            .insert(receiver.to_string(), now);
        let queue = self.amq.pending.get(receiver);
        let depth = queue.map_or(0, VecDeque::len);
        let preview = queue
            .and_then(VecDeque::front)
            .map(|h| queue::preview(&h.body, 80))
            .unwrap_or_default();
        let marker = match reason {
            HoldReason::BusyMarker(m) => format!(" marker={m:?}"),
            _ => String::new(),
        };
        crate::logger::debug(&format!(
            "amq: holding {receiver} reason={}{marker} depth={depth} head={preview}",
            reason.as_str()
        ));
    }

    fn warn_no_session(&mut self, receiver: &str, now: Instant, out: &mut Vec<EventReaction>) {
        let due = self
            .amq
            .last_warned
            .get(receiver)
            .is_none_or(|t| now.duration_since(*t) >= WARN_RATE_LIMIT);
        if !due {
            return;
        }
        self.amq.last_warned.insert(receiver.to_string(), now);
        let count = self.amq.pending.get(receiver).map_or(0, VecDeque::len);
        out.push(EventReaction::Status(StatusUpdate::warning(format!(
            "AMQ inject: {count} message(s) queued for receiver \"{receiver}\" but no agent matches its handle. Start an agent with that handle, or move the files out of the inject-queue."
        ))));
    }

    fn warn_timeout(
        &mut self,
        receiver: &str,
        now: Instant,
        timeout: Duration,
        out: &mut Vec<EventReaction>,
    ) {
        if timeout.is_zero() {
            return;
        }
        if delivery::timeout_warning_due(
            &mut self.amq.first_pending_at,
            &mut self.amq.timeout_warned,
            receiver,
            now,
            timeout,
        ) {
            self.amq.last_warned.insert(receiver.to_string(), now);
            out.push(EventReaction::Status(StatusUpdate::warning(format!(
                "AMQ inject: messages for \"{receiver}\" still pending after {}s; agent stayed busy or no agent matched.",
                timeout.as_secs()
            ))));
        }
    }

    fn find_session_for_receiver(&self, receiver: &str) -> Option<String> {
        let handles: Vec<String> = self.sessions.iter().map(agent_handle_of).collect();
        let candidates: Vec<ReceiverCandidate<'_>> = self
            .sessions
            .iter()
            .zip(&handles)
            .map(|(s, h)| ReceiverCandidate {
                id: s.id.as_str(),
                handle: h.as_str(),
                branch: s.branch_name().unwrap_or(""),
                directory: s.directory(),
            })
            .collect();
        delivery::match_receiver(&candidates, receiver).map(str::to_string)
    }

    /// The agent's session-slot PTY, if running. AMQ targets the agent, and
    /// the slot tab is the agent's own pane.
    fn slot_client(&self, session_id: &str) -> Option<&crate::pty::PtyClient> {
        let session = self.session_by_id(session_id)?;
        self.providers
            .get(session.slot_tab_id())
            .filter(|c| c.is_live())
    }

    fn running_provider_of(&self, session_id: &str) -> Option<crate::model::ProviderKind> {
        self.session_by_id(session_id)
            .map(|s| self.running_provider_for(s))
    }

    // ─── orchestrator watchdog ───────────────────────────────────────

    fn flush_amq_pending_enters(&mut self) {
        if self.amq.pending_enters.is_empty() {
            return;
        }
        let now = Instant::now();
        let delay = delivery::effective_enter_phase_delay(self.config.amq.inject.phase_delay_ms);
        let due: Vec<String> = self
            .amq
            .pending_enters
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= delay)
            .map(|(id, _)| id.clone())
            .collect();
        for session_id in due {
            self.amq.pending_enters.remove(&session_id);
            let submit =
                delivery::submit_key_bytes_for_provider(self.running_provider_of(&session_id).as_ref());
            match self.slot_client(&session_id) {
                Some(client) => {
                    if let Err(err) = client.write_bytes(submit) {
                        crate::logger::warn(&format!(
                            "amq: deferred Enter failed for {session_id}: {err}"
                        ));
                    }
                }
                None => crate::logger::debug("amq: deferred Enter skipped, PTY gone"),
            }
        }
    }

    fn tick_orchestrator_watchdog(&mut self, focus: AmqFocus<'_>) {
        let ocfg = self.config.amq.orchestrator.clone();
        if !ocfg.enabled || ocfg.poll_interval_secs == 0 {
            return;
        }
        let icfg = self.config.amq.inject.clone();
        let now = Instant::now();
        let poll_interval = Duration::from_secs(ocfg.poll_interval_secs);
        let startup_grace = Duration::from_millis(icfg.startup_grace_ms);
        let orchestrators: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| self.session_is_live(s) && self.session_mode(&s.id) == ContextMode::Orchestrator)
            .map(|s| s.id.clone())
            .collect();
        let live: HashSet<&String> = orchestrators.iter().collect();
        self.amq.orchestrator_last_nudged.retain(|id, _| live.contains(id));
        self.amq.orchestrator_policy_injected.retain(|id| live.contains(id));
        let live_projects: HashSet<String> = self
            .sessions
            .iter()
            .filter(|s| live.contains(&s.id))
            .map(|s| s.project_id().unwrap_or("").to_string())
            .collect();
        self.amq
            .orchestrator_project_last_checkpoint
            .retain(|p, _| live_projects.contains(p));
        if orchestrators.is_empty() {
            return;
        }

        for session_id in orchestrators {
            let Some(session) = self.session_by_id(&session_id) else {
                continue;
            };
            let receiver = amq_receiver_for_session(session);
            let provider = self.running_provider_for(session).as_str().to_string();
            let project_key = session.project_id().unwrap_or("").to_string();
            let startup_policy = self
                .session_settings_or_default(&session_id)
                .effective_system_prompt()
                .unwrap_or_else(|| ORCHESTRATOR_SYSTEM_PROMPT.to_string());
            let peers = self.orchestrator_peers_for(&session_id);
            let needs_startup_policy = !self.amq.orchestrator_policy_injected.contains(&session_id)
                && orch::provider_needs_pty_orchestrator_policy(&provider);

            let prompt = if needs_startup_policy {
                let first_seen = *self
                    .amq
                    .orchestrator_last_nudged
                    .entry(session_id.clone())
                    .or_insert(now);
                if now.duration_since(first_seen) < startup_grace {
                    continue;
                }
                orch::build_orchestrator_startup_policy_prompt(&startup_policy, &peers)
            } else {
                self.amq.orchestrator_policy_injected.insert(session_id.clone());
                let Some(last) = self.amq.orchestrator_last_nudged.get(&session_id).copied() else {
                    // First sighting starts the clock; never poll at startup.
                    self.amq.orchestrator_last_nudged.insert(session_id, now);
                    continue;
                };
                if now.duration_since(last) < poll_interval
                    || self
                        .amq
                        .orchestrator_project_last_checkpoint
                        .get(&project_key)
                        .is_some_and(|t| now.duration_since(*t) < poll_interval)
                    || peers.is_empty()
                {
                    continue;
                }
                orch::resolve_orchestrator_checkpoint_prompt(&ocfg.checkpoint_prompt, &peers)
            };

            // Same idle/busy safeguards as AMQ inject.
            if self.amq.pending_enters.contains_key(&session_id)
                || self.amq.pending.get(&receiver).is_some_and(|q| !q.is_empty())
                || self
                    .amq
                    .cooldown_until
                    .get(&session_id)
                    .is_some_and(|until| now < *until)
            {
                continue;
            }
            if focus.focused_session == Some(session_id.as_str()) {
                let last = self.amq.last_user_keystroke.get(&session_id).copied();
                if delivery::should_hold_for_quiet_window(
                    last,
                    now,
                    Duration::from_secs(icfg.active_session_quiet_secs),
                ) {
                    continue;
                }
            }
            let Some(client) = self.slot_client(&session_id) else {
                continue;
            };
            if queue::snapshot_busy_marker(
                &client.scan_recent_lines(icfg.busy_scan_lines.max(1)),
                &icfg.busy_markers,
            )
            .is_some()
            {
                continue;
            }
            let payload = delivery::inject_body_bytes_for_provider(
                &prompt,
                self.running_provider_of(&session_id).as_ref(),
            );
            match client.write_bytes(&payload) {
                Ok(()) => {
                    self.amq.pending_enters.insert(session_id.clone(), now);
                    self.amq
                        .orchestrator_last_nudged
                        .insert(session_id.clone(), now);
                    if needs_startup_policy {
                        self.amq.orchestrator_policy_injected.insert(session_id.clone());
                    } else {
                        self.amq
                            .orchestrator_project_last_checkpoint
                            .insert(project_key.clone(), now);
                    }
                    crate::logger::info(&format!(
                        "amq: sent orchestrator {} to {session_id} (peers={})",
                        if needs_startup_policy { "startup policy" } else { "checkpoint" },
                        peers.len()
                    ));
                }
                Err(err) => crate::logger::warn(&format!(
                    "amq: orchestrator prompt write failed for {session_id}: {err}"
                )),
            }
        }
    }

    fn session_is_live(&self, session: &AgentSession) -> bool {
        session.status == SessionStatus::Active
            && self
                .providers
                .get(session.slot_tab_id())
                .is_some_and(crate::pty::PtyClient::is_live)
    }

    fn orchestrator_peers_for(&self, orchestrator_id: &str) -> Vec<OrchestratorPeer> {
        let Some(orchestrator) = self.session_by_id(orchestrator_id) else {
            return Vec::new();
        };
        let project = orchestrator.project_id();
        self.sessions
            .iter()
            .filter(|s| {
                orch::is_orchestrator_checkpoint_peer(
                    &s.id,
                    s.project_id(),
                    self.session_mode(&s.id),
                    self.session_is_live(s),
                    orchestrator_id,
                    project,
                )
            })
            .map(|s| OrchestratorPeer {
                handle: amq_receiver_for_session(s),
                label: s.display_label(),
                provider: self.running_provider_for(s).as_str().to_string(),
                mode: self.session_mode(&s.id),
                branch: s.branch_name().unwrap_or("").to_string(),
                worktree: s.directory().to_string(),
            })
            .collect()
    }
}

/// Status line summary for a settings save (fork
/// `build_session_settings_save_summary`).
pub fn build_session_settings_save_summary(
    mode_changed: bool,
    title_changed: bool,
    auto_clear_changed: bool,
    system_prompt_changed: bool,
    needs_respawn: bool,
) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if title_changed {
        parts.push("title");
    }
    if mode_changed {
        parts.push("context mode");
    }
    if auto_clear_changed {
        parts.push("auto-clear");
    }
    if system_prompt_changed {
        parts.push("system prompt");
    }
    if needs_respawn {
        parts.push("spawn-time settings");
    }
    if parts.is_empty() {
        "Session settings saved (no changes).".to_string()
    } else {
        format!("Session settings saved: {}.", parts.join(", "))
    }
}

/// Trim the system prompt's trailing whitespace and treat an all-blank one
/// as unset, so the store never carries `""` (fork save path).
fn normalize_settings(mut settings: SessionSettings) -> SessionSettings {
    settings.system_prompt = settings
        .system_prompt
        .map(|p| p.trim_end().to_string())
        .filter(|p| !p.trim().is_empty());
    settings
}

fn max_message_age(secs: u64) -> Option<Duration> {
    (secs != 0).then(|| Duration::from_secs(secs))
}

fn reaction_from(mut reactions: Vec<EventReaction>) -> EventReaction {
    match reactions.len() {
        0 => EventReaction::Nothing,
        1 => reactions.pop().expect("one"),
        _ => EventReaction::Multi(reactions),
    }
}

/// The agent's AMQ handle.
///
/// INTEGRATION: return `session.agent_handle()` once the shared-workspace
/// worker (evergreen) adds the immutable handle to `AgentSession`. Until
/// then this is the fork's pre-handle derivation (worktree basename, then
/// branch, then id), which is what the wrappers compute for a dux pane.
pub fn agent_handle_of(session: &AgentSession) -> String {
    amq_receiver_for_session(session)
}

/// The receiver name a session's wrapper derives (fork
/// `amq_receiver_for_session`).
pub fn amq_receiver_for_session(session: &AgentSession) -> String {
    // INTEGRATION: prefer `session.agent_handle()` (evergreen).
    if let Some(base) = std::path::Path::new(session.directory())
        .file_name()
        .and_then(|n| n.to_str())
    {
        let r = delivery::sanitise_handle(base);
        if !r.is_empty() {
            return r;
        }
    }
    let r = delivery::sanitise_handle(session.branch_name().unwrap_or(""));
    if r.is_empty() { session.id.clone() } else { r }
}

/// AMQ's shared root, for the collaboration guard.
///
/// INTEGRATION: the shared-workspace / peer workers own AMQ root
/// resolution; switch to their resolver at merge.
fn amq_root_for_collaboration_guard() -> PathBuf {
    std::env::var_os("AM_ROOT")
        .or_else(|| std::env::var_os("AMQ_GLOBAL_ROOT"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/state/amq"))
}
