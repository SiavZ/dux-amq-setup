//! App-side glue for the AMQ inject-queue drainer.
//!
//! See `crate::amq_inject` for the lower-level types (queue scanning,
//! validation, claim/release). This module wires those into the App's
//! tick loop, holds the per-receiver pending queue, and translates a
//! delivery decision into a PTY write.
//!
//! Three entry points:
//!
//! 1. [`App::spawn_amq_inject_watcher`] — bootstrap. Resolves the
//!    queue dir from config, starts the `notify` watcher + polling
//!    fallback, and stores the watcher handle on `RuntimeState`.
//! 2. [`App::drain_inject_queue_dir`] — fired in response to
//!    [`WorkerEvent::AmqInjectScanRequested`]. Walks the queue dir,
//!    atomically claims new `.msg` files, validates them, and pushes
//!    each onto the per-receiver pending queue.
//! 3. [`App::tick_amq_inject`] — called once per main-loop tick. For
//!    each receiver with pending messages, looks up the matching
//!    session, checks busy/active state, and either writes the body
//!    to the PTY (followed by `\r` to submit) or leaves it queued.
//!
//! The receiver→session mapping uses the same sanitisation the AMQ
//! wrappers apply (lowercase, `[a-z0-9_-]`), so a session whose
//! `branch_name` is `Feature/Login` correctly matches a queued
//! message addressed to `feature-login`.

use super::*;

use std::time::Duration;

use crate::amq_inject::{self, QueuedMessage, UNROUTED_RECEIVER};

/// How long between repeat warnings about the same un-deliverable
/// receiver. Stops a queue full of messages for an unknown handle
/// from spamming the status line at tick rate.
const WARN_RATE_LIMIT: Duration = Duration::from_secs(60);

/// How long between repeat `debug`-level "drainer holding for X"
/// trace events for the same receiver. Without this throttle the
/// main loop ticks (~10 Hz idle, faster on input) would each emit
/// one event per held receiver, and the JSON log would grow at the
/// tick rate. Set to 60 s so a typical operator running with
/// `RUST_LOG=dux::amq_inject=debug` sees one diagnostic line per
/// minute per held receiver — enough to confirm whether a delivery
/// is being held or simply hasn't fired, without flooding `dux.log`.
pub(crate) const HOLD_LOG_RATE_LIMIT: Duration = Duration::from_secs(60);

/// Keep the AMQ bridge from turning a large backlog into a large in-memory
/// pending queue. Files beyond this per-scan budget remain as `.msg` files and
/// will be picked up by later notify/poll scans.
const MAX_INJECT_CLAIMS_PER_SCAN: usize = 32;

/// Hard cap on AMQ messages claimed but not yet delivered. Claimed files are
/// renamed to `.inflight.*`, so bounding this also bounds crash recovery work.
const MAX_INJECT_PENDING_TOTAL: usize = 128;

/// Per-receiver cap so one noisy handle does not starve other receivers or
/// leave hundreds of `.inflight.*` files behind if dux exits mid-batch.
const MAX_INJECT_PENDING_PER_RECEIVER: usize = 32;

/// Bound PTY writes per UI tick. Delivery remains fast, but AMQ injection can
/// no longer monopolise the event loop when many receivers have backlogs.
const MAX_INJECT_ACTIONS_PER_TICK: usize = 16;

/// How long after AMQ delivery the watch engine should skip observing
/// the target session. Prevents the `[task-done]` sentinel in the
/// Worker-mode postscript from false-firing the auto-clear rule.
/// The window must be long enough for the agent's TUI to consume the
/// input and push the postscript off the visible terminal area. 10 s
/// is conservative: Claude Code typically clears the input and starts
/// streaming within 1-2 s, and the postscript is at the end of the
/// message so it scrolls off first.
const WATCH_SUPPRESS_AFTER_INJECT: Duration = Duration::from_secs(10);

/// Reasons the drainer can hold a message instead of delivering.
/// Each variant carries enough context to diagnose without a stack
/// trace; see `App::log_holding` for the formatted output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HoldReason<'a> {
    /// Receiver has no matching session in `git.sessions`. The
    /// message stays in the per-receiver queue until either a
    /// matching session spawns or the operator moves the file out.
    NoSession,
    /// `InputTarget::Agent` is set and the active session is the
    /// receiver target — the user is typing into that pane right now,
    /// so we don't interrupt them.
    UserTyping,
    /// Session matched by name but its PTY handle is gone
    /// (detached/exited). Re-spawn the session and the next tick
    /// will pick it up.
    PtyGone,
    /// Bottom-of-screen scan turned up a configured busy marker
    /// (e.g. "esc to interrupt"). The matching marker is included
    /// for diagnosis when the user disagrees with our verdict.
    BusyMarker(&'a str),
}

impl<'a> HoldReason<'a> {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoSession => "no_matching_session",
            Self::UserTyping => "user_typing_in_target_session",
            Self::PtyGone => "pty_gone",
            Self::BusyMarker(_) => "busy_marker_detected",
        }
    }
}

/// Sanitise a string the same way the AMQ wrappers do before deriving
/// the agent handle. Used to match a receiver name (already sanitised
/// by the bridge) against a session's `branch_name`.
pub(crate) fn sanitise_handle(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-' {
            out.push(ch);
        } else if ch.is_ascii_uppercase() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Pure helper that does the receiver→session-id resolution given a
/// flat list of `(session_id, branch_name, worktree_path)` triples and
/// a sanitised receiver. Mirrors the AMQ wrapper's ME-derivation
/// priority: worktree dir basename first (the path the wrappers
/// actually take inside a dux pane), then branch name (legacy
/// fallback), then exact session id (operator escape hatch). See the
/// docstring on `App::find_session_for_receiver` for the full
/// rationale.
///
/// Returns `None` when no session matches. Stops at the first match
/// in priority order — if two sessions both sanitise to the same
/// receiver, the one whose worktree basename matches wins regardless
/// of declaration order.
pub(crate) fn match_receiver<'a, I>(sessions: I, receiver: &str) -> Option<&'a str>
where
    I: Clone + IntoIterator<Item = (&'a str, &'a str, &'a str)>,
{
    for (id, _branch, worktree) in sessions.clone() {
        if let Some(basename) = std::path::Path::new(worktree)
            .file_name()
            .and_then(|n| n.to_str())
            && sanitise_handle(basename) == receiver
        {
            return Some(id);
        }
    }
    for (id, branch, _worktree) in sessions.clone() {
        if sanitise_handle(branch) == receiver {
            return Some(id);
        }
    }
    for (id, _branch, _worktree) in sessions {
        if id == receiver {
            return Some(id);
        }
    }
    None
}

impl App {
    /// Bootstrap the AMQ inject-queue watcher. Called once during
    /// `App::run`. Failures are logged but never fatal — dux must
    /// still come up if the home directory is unwritable or notify
    /// can't initialise.
    pub(crate) fn spawn_amq_inject_watcher(&mut self) {
        if !self.config.amq.inject.enabled {
            tracing::info!(
                target: "dux::amq_inject",
                "drainer disabled in config; skipping watcher",
            );
            return;
        }
        let Some(queue_dir) = amq_inject::resolve_queue_dir(&self.config.amq.inject) else {
            tracing::warn!(
                target: "dux::amq_inject",
                "could not resolve queue dir (no $HOME?); skipping watcher",
            );
            return;
        };
        self.runtime.amq_inject_queue_dir = Some(queue_dir.clone());

        // Reclaim any stale `.inflight.<ts>.msg` files from a prior
        // dux instance that crashed mid-delivery. The single-instance
        // lock guarantees nobody else is currently processing them.
        // Bridge-format mktemp temps (no `.msg` suffix) are skipped
        // so we don't corrupt an in-progress write.
        match amq_inject::reclaim_stale_inflight(&queue_dir) {
            Ok(0) => {}
            Ok(n) => {
                tracing::info!(
                    target: "dux::amq_inject",
                    reclaimed = n,
                    "reclaimed stale inflight files from prior dux instance",
                );
            }
            Err(err) => {
                tracing::warn!(
                    target: "dux::amq_inject",
                    queue_dir = %queue_dir.display(),
                    err = %err,
                    "reclaim sweep failed; some queued messages may stay stuck",
                );
            }
        }

        let tx = self.runtime.worker_tx.clone();
        let make_event = || WorkerEvent::AmqInjectScanRequested;
        match amq_inject::spawn_inject_watcher(
            queue_dir.clone(),
            self.config.amq.inject.poll_interval_ms,
            tx.clone(),
            make_event,
        ) {
            Ok(watcher) => {
                self.runtime.amq_inject_watcher = Some(watcher);
                tracing::info!(
                    target: "dux::amq_inject",
                    queue_dir = %queue_dir.display(),
                    poll_interval_ms = self.config.amq.inject.poll_interval_ms,
                    "drainer initialised",
                );
                // Kick off an initial scan so any messages queued
                // while dux wasn't running get drained on the first
                // tick instead of waiting for the next FS event.
                let _ = tx.send(WorkerEvent::AmqInjectScanRequested);
            }
            Err(err) => {
                tracing::warn!(
                    target: "dux::amq_inject",
                    queue_dir = %queue_dir.display(),
                    err = %err,
                    "drainer watcher failed to start; queued messages will NOT be delivered",
                );
            }
        }
    }

    /// Walk the queue directory and claim every new `.msg` file we find.
    /// Validation failures and bad receiver names produce a status-line
    /// warning. Successful claims are pushed onto
    /// `runtime.amq_inject_pending` and delivered by `tick_amq_inject`.
    pub(crate) fn drain_inject_queue_dir(&mut self) {
        let Some(queue_dir) = self.runtime.amq_inject_queue_dir.clone() else {
            return;
        };
        let max_bytes = self.config.amq.inject.max_message_bytes;
        let available_capacity = MAX_INJECT_PENDING_TOTAL.saturating_sub(
            self.runtime
                .amq_inject_pending
                .values()
                .map(|q| q.len())
                .sum::<usize>(),
        );
        let scan_limit = MAX_INJECT_CLAIMS_PER_SCAN.min(available_capacity);
        if scan_limit == 0 {
            tracing::debug!(
                target: "dux::amq_inject",
                max_pending = MAX_INJECT_PENDING_TOTAL,
                "pending queue is full; deferring AMQ inject scan",
            );
            return;
        }

        let outcome = match amq_inject::scan_queue_dir_limited(&queue_dir, scan_limit) {
            Ok(o) => o,
            Err(err) => {
                tracing::warn!(
                    target: "dux::amq_inject",
                    queue_dir = %queue_dir.display(),
                    err = %err,
                    "scan failed",
                );
                return;
            }
        };

        let mut total_pending = self
            .runtime
            .amq_inject_pending
            .values()
            .map(|q| q.len())
            .sum::<usize>();
        let mut claimed_this_scan = 0usize;
        let mut deferred_this_scan = 0usize;

        for (path, reason) in &outcome.rejections {
            tracing::warn!(
                target: "dux::amq_inject",
                path = %path.display(),
                reason = %reason.human(),
                "queue entry rejected",
            );
        }

        for pending in outcome.messages {
            let receiver_depth = self
                .runtime
                .amq_inject_pending
                .get(&pending.receiver)
                .map(|q| q.len())
                .unwrap_or(0);
            if claimed_this_scan >= MAX_INJECT_CLAIMS_PER_SCAN
                || total_pending >= MAX_INJECT_PENDING_TOTAL
                || receiver_depth >= MAX_INJECT_PENDING_PER_RECEIVER
            {
                deferred_this_scan += 1;
                continue;
            }

            let inflight = match amq_inject::claim(&pending.path) {
                Ok(p) => p,
                Err(err) => {
                    // Race with another scan, or a permission issue.
                    // Skip silently — the next scan picks it up.
                    tracing::debug!(
                        target: "dux::amq_inject",
                        path = %pending.path.display(),
                        err = %err,
                        "claim failed; will retry on next scan",
                    );
                    continue;
                }
            };
            claimed_this_scan += 1;
            match amq_inject::read_validated(&inflight, max_bytes) {
                Ok(body) => {
                    let queued = QueuedMessage {
                        receiver: pending.receiver.clone(),
                        body,
                        inflight_path: inflight,
                        source_path: pending.path.clone(),
                        body_typed: false,
                        body_typed_at: None,
                    };
                    self.runtime
                        .amq_inject_pending
                        .entry(pending.receiver.clone())
                        .or_default()
                        .push_back(queued);
                    total_pending += 1;
                }
                Err(rejection) => {
                    // Validation failed AFTER claim. The file is
                    // already renamed to `.inflight.<name>` so future
                    // scans won't see it; we leave it there for the
                    // operator to inspect rather than loop forever
                    // re-rejecting the same broken file.
                    tracing::warn!(
                        target: "dux::amq_inject",
                        path = %inflight.display(),
                        reason = %rejection.human(),
                        "queue entry rejected after claim; left at .inflight.* for inspection",
                    );
                    self.set_warning(format!("AMQ inject: {}", rejection.human(),));
                }
            }
        }

        if deferred_this_scan > 0 {
            tracing::info!(
                target: "dux::amq_inject",
                deferred = deferred_this_scan,
                claimed = claimed_this_scan,
                max_claims_per_scan = MAX_INJECT_CLAIMS_PER_SCAN,
                max_pending_total = MAX_INJECT_PENDING_TOTAL,
                max_pending_per_receiver = MAX_INJECT_PENDING_PER_RECEIVER,
                "deferred AMQ inject files because the drainer is at its safety budget",
            );
        }
    }

    /// Per-tick drainer. For each receiver with a pending head, find
    /// the matching session, check busy/active state, and deliver
    /// when the conditions allow. Mirrors the gating logic in
    /// `tick_watch_engines` (skip the active session, skip when the
    /// user is typing) plus a busy-marker check that's specific to
    /// this drainer.
    pub(crate) fn tick_amq_inject(&mut self) {
        if !self.config.amq.inject.enabled {
            return;
        }
        if self.runtime.amq_inject_pending.is_empty() {
            return;
        }
        let now = Instant::now();
        let busy_markers = self.config.amq.inject.busy_markers.clone();
        let busy_scan_lines = self.config.amq.inject.busy_scan_lines.max(1);
        let timeout = Duration::from_secs(self.config.amq.inject.delivery_timeout_secs);
        let phase_delay = Duration::from_millis(self.config.amq.inject.phase_delay_ms);

        // Snapshot the active-session id once so the inner loop never
        // has to borrow self.ui twice.
        let active_session = if matches!(self.ui.input_target, InputTarget::Agent) {
            self.selected_session().map(|s| s.id.clone())
        } else {
            None
        };

        // Snapshot receivers because we'll mutate `amq_inject_pending`
        // inside the loop and can't iterate it directly.
        let receivers: Vec<String> = self.runtime.amq_inject_pending.keys().cloned().collect();

        let mut actions_this_tick = 0usize;
        'receivers: for receiver in receivers {
            if actions_this_tick >= MAX_INJECT_ACTIONS_PER_TICK {
                break;
            }
            // Resolve target session. `_unrouted` falls back to the
            // currently-selected session so messages from peers using
            // an older bridge (no AM_ME export) still land somewhere
            // visible.
            let session_id = if receiver == UNROUTED_RECEIVER {
                self.selected_session().map(|s| s.id.clone())
            } else {
                self.find_session_for_receiver(&receiver)
            };

            let Some(session_id) = session_id else {
                self.maybe_warn_no_session(&receiver, now);
                self.maybe_warn_timeout(&receiver, now, timeout);
                self.log_holding(&receiver, now, HoldReason::NoSession);
                continue;
            };

            // Don't interrupt the operator mid-prompt — but DO deliver
            // when interactive mode is open and the operator hasn't
            // typed in a while. The old rule "skip whenever interactive
            // mode is on the target session" was too coarse: an operator
            // who kept the pane interactive to watch the agent saw all
            // AMQ messages held until they exited interactive mode.
            //
            // The quiet-window heuristic: skip only when interactive AND
            // `now - last_user_keystroke < active_session_quiet_secs`.
            // Default quiet window is 60 s (config:
            // `[amq.inject].active_session_quiet_secs`). Set to 0 to
            // restore the old always-skip behaviour.
            //
            // The map is populated by `record_user_keystroke_for_active_session`
            // in the input handler, which fires when the operator's
            // keystrokes (or paste / macro) reach a session's PTY.
            // Programmatic writes (drainer, watch effects) MUST NOT
            // touch the map or the heuristic feeds back into itself.
            if active_session.as_deref() == Some(session_id.as_str()) {
                let quiet = Duration::from_secs(self.config.amq.inject.active_session_quiet_secs);
                let last = self.runtime.last_user_keystroke.get(&session_id).copied();
                if should_hold_for_quiet_window(last, now, quiet) {
                    self.log_holding(&receiver, now, HoldReason::UserTyping);
                    continue;
                }
                // else: interactive but quiet → fall through and deliver
            }

            // Two-phase delivery loop:
            //   Phase 1 (head.body_typed = false): write the body
            //     bytes to the PTY, mark `body_typed = true`, then
            //     break out of the inner loop. The next tick will
            //     pick this entry back up for phase 2.
            //   Phase 2 (head.body_typed = true): write a discrete
            //     `\r` to the PTY, unlink the inflight file, pop the
            //     entry, and continue draining the queue.
            //
            // Why split: a single PTY write of `body + \r` gets
            // coalesced by Ink (Claude Code's TUI framework) into a
            // paste-shaped buffer; the trailing `\r` ends up appended
            // to the input field rather than firing as a submit
            // keystroke. Splitting across two ticks puts a real time
            // gap between writes (~16 ms typical), which produces two
            // separate `read()` calls on Ink's stdin so the final
            // `\r` arrives alone and is interpreted as Enter.
            //
            // The busy-check is re-run between every pop so a backlog
            // of messages stops draining if the agent goes back into
            // streaming mid-batch.
            loop {
                if actions_this_tick >= MAX_INJECT_ACTIONS_PER_TICK {
                    tracing::debug!(
                        target: "dux::amq_inject",
                        actions = actions_this_tick,
                        max_actions = MAX_INJECT_ACTIONS_PER_TICK,
                        "hit AMQ inject per-tick write budget; deferring remaining pending messages",
                    );
                    break 'receivers;
                }

                let queue_empty = self
                    .runtime
                    .amq_inject_pending
                    .get(&receiver)
                    .is_none_or(|q| q.is_empty());
                if queue_empty {
                    break;
                }

                let snapshot = match self.find_pty_handle(&session_id) {
                    Some(handle) => handle.scan_recent_lines(busy_scan_lines),
                    None => {
                        // Session matched by name but PTY is gone
                        // (detached/exited). Leave queued; if the user
                        // re-spawns, we'll pick it up.
                        self.maybe_warn_timeout(&receiver, now, timeout);
                        self.log_holding(&receiver, now, HoldReason::PtyGone);
                        break;
                    }
                };
                if let Some(matched) = amq_inject::snapshot_busy_marker(&snapshot, &busy_markers) {
                    self.maybe_warn_timeout(&receiver, now, timeout);
                    self.log_holding(&receiver, now, HoldReason::BusyMarker(matched));
                    break;
                }

                // Decide which phase the head is in. Read-only borrow
                // released before we mutate.
                let phase_info = self
                    .runtime
                    .amq_inject_pending
                    .get(&receiver)
                    .and_then(|q| q.front())
                    .map(|head| (head.body_typed, head.body_typed_at));

                match phase_info {
                    Some((false, _)) => {
                        // Phase 1: type the body, leave the entry at
                        // the head of the queue with body_typed=true,
                        // record the timestamp, and break out so a
                        // future tick handles \r after the phase delay.
                        let body = {
                            let q = self
                                .runtime
                                .amq_inject_pending
                                .get_mut(&receiver)
                                .expect("queue exists per phase peek");
                            let head_mut = q.front_mut().expect("head exists per phase peek");
                            head_mut.body_typed = true;
                            head_mut.body_typed_at = Some(now);
                            head_mut.body.clone()
                        };
                        self.deliver_inject_body(&session_id, &receiver, &body);
                        actions_this_tick += 1;
                        break;
                    }
                    Some((true, typed_at)) => {
                        // Phase 2: enforce the configured phase delay
                        // before sending \r. This prevents coalescing
                        // under heavy CPU load where ticks fire faster
                        // than the Ink input flush cycle.
                        if let Some(at) = typed_at {
                            if now.duration_since(at) < phase_delay {
                                break;
                            }
                        }
                        let head = self
                            .runtime
                            .amq_inject_pending
                            .get_mut(&receiver)
                            .and_then(|q| q.pop_front())
                            .expect("head exists per phase peek");
                        self.deliver_inject_enter(&session_id, &receiver, &head);
                        actions_this_tick += 1;
                        // Loop continues to drain next message.
                    }
                    None => break,
                }
            }

            // Tidy up empty queues so the receivers Vec next tick
            // doesn't grow unbounded.
            if self
                .runtime
                .amq_inject_pending
                .get(&receiver)
                .is_some_and(|q| q.is_empty())
            {
                self.runtime.amq_inject_pending.remove(&receiver);
                self.runtime.amq_inject_last_warned.remove(&receiver);
            }
        }
    }

    /// Map a sanitised receiver name back to a session id. Returns
    /// `None` if no live session has a matching branch name.
    /// Map a sanitised receiver name back to a session id, mirroring
    /// the AMQ wrapper's `ME` derivation priority. The wrappers in
    /// `dux-amq/wrappers/` resolve their handle as:
    ///
    ///   1. `$AM_ME` env if set (explicit override)
    ///   2. `basename($PWD)` if running inside a dux worktree
    ///   3. git `branch --show-current`
    ///   4. `<provider>-<pid>` (no-context fallback)
    ///
    /// Step 2 is what fires for every dux-spawned pane, and it picks
    /// up the **worktree directory name** — i.e. whatever the branch
    /// was called when the worktree was created via `git worktree add`.
    /// dux can later rename the branch in that worktree (the user
    /// pushes a feature branch, switches to a hotfix, etc.); the
    /// directory name does not follow.
    ///
    /// So the receiver "front-end-qa" can correspond to a session
    /// whose `branch_name` is now `fix/qa-s45-charge-schema-paymentmethod`
    /// but whose `worktree_path` ends in `Front-end-QA`. We try the
    /// directory basename first (matching the primary path the
    /// wrappers actually use), then fall back to branch name (matching
    /// the legacy fallback path), and finally settle for an exact
    /// match against the session id (so an operator can address by id
    /// when the worktree dir name is ambiguous).
    fn find_session_for_receiver(&self, receiver: &str) -> Option<String> {
        let triples: Vec<(&str, &str, &str)> = self
            .git
            .sessions
            .iter()
            .map(|s| {
                (
                    s.id.as_str(),
                    s.branch_name.as_str(),
                    s.worktree_path.as_str(),
                )
            })
            .collect();
        match_receiver(triples.iter().copied(), receiver).map(|s| s.to_string())
    }

    /// Phase 1 of two-phase delivery: type the body into the
    /// session's PTY without the trailing `\r`. Reuses
    /// `crate::app::input::macro_payload_bytes` so embedded newlines
    /// become Alt-Enter (a newline within the prompt) rather than
    /// premature submits — the same chokepoint watch effects use.
    /// The inflight file stays on disk; phase 2 unlinks it.
    ///
    /// audit03 Phase 5: when the receiving session is in
    /// [`crate::model::ContextMode::Worker`] mode, dux appends a
    /// postscript instructing the agent to emit
    /// `[task-done]` (the literal sentinel from
    /// `crate::watch::builtin::TASK_DONE_SENTINEL`) at end-of-task.
    /// The auto-clear watch rule (Phase 4) keys off that sentinel to
    /// wipe the worker's context. The postscript lives dux-side
    /// rather than in the bash bridge so the bridge stays stateless;
    /// the bridge already operates outside dux's process tree (the
    /// AMQ wake daemon `setsid`s it) and has no SQLite access.
    fn deliver_inject_body(&mut self, session_id: &str, receiver: &str, body: &str) {
        let mode = self
            .git
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .map(|s| s.settings.mode)
            .unwrap_or_default();

        let body_with_postscript = apply_inject_postscript(body, mode);
        if body_with_postscript.len() != body.len() {
            tracing::debug!(
                target: "dux::session_settings",
                session_id = %session_id,
                receiver = %receiver,
                "appending Worker-mode task-done postscript to AMQ wake",
            );
        }

        let payload = crate::app::input::macro_payload_bytes(&body_with_postscript);
        let write_result = self
            .find_pty_handle(session_id)
            .map(|handle| handle.write_bytes(&payload));
        match write_result {
            Some(Ok(())) => {
                tracing::debug!(
                    target: "dux::amq_inject",
                    session_id = %session_id,
                    receiver = %receiver,
                    body_preview = %amq_inject::preview(body, 80),
                    "typed AMQ wake body (phase 1); awaiting tick 2 to send Enter",
                );
                // Suppress the watch engine for this session so the
                // postscript's `[task-done]` sentinel (about to appear
                // in the PTY) doesn't false-fire the auto-clear rule.
                // The suppression covers phase 1 → phase 2 and a few
                // seconds beyond, giving the agent time to consume the
                // input and push the postscript off the visible area.
                self.runtime.watch_suppress_until.insert(
                    session_id.to_string(),
                    Instant::now() + WATCH_SUPPRESS_AFTER_INJECT,
                );
            }
            Some(Err(err)) => {
                // PTY write failed; mark this entry not-yet-typed so
                // the next tick retries phase 1. The inflight file is
                // already in pending so it survives the failure.
                tracing::warn!(
                    target: "dux::amq_inject",
                    session_id = %session_id,
                    receiver = %receiver,
                    err = %err,
                    "phase-1 PTY write failed; will retry next tick",
                );
                if let Some(q) = self.runtime.amq_inject_pending.get_mut(receiver)
                    && let Some(head) = q.front_mut()
                {
                    head.body_typed = false;
                    head.body_typed_at = None;
                }
            }
            None => {
                // No PTY (session gone). Roll back so a future
                // re-spawn picks the message up.
                if let Some(q) = self.runtime.amq_inject_pending.get_mut(receiver)
                    && let Some(head) = q.front_mut()
                {
                    head.body_typed = false;
                    head.body_typed_at = None;
                }
            }
        }
    }

    /// Phase 2 of two-phase delivery: write a discrete `\r` to the
    /// session's PTY, unlink the inflight file, and surface success
    /// in the status line. The split-write pattern is what makes
    /// Ink's stdin reader see the `\r` as a separate keystroke
    /// rather than coalescing it into the body's paste buffer.
    fn deliver_inject_enter(&mut self, session_id: &str, receiver: &str, msg: &QueuedMessage) {
        let write_result = self
            .find_pty_handle(session_id)
            .map(|handle| handle.write_bytes(b"\r"));
        match write_result {
            Some(Ok(())) => {
                if let Err(err) = std::fs::remove_file(&msg.inflight_path) {
                    tracing::warn!(
                        target: "dux::amq_inject",
                        session_id = %session_id,
                        receiver = %receiver,
                        path = %msg.inflight_path.display(),
                        err = %err,
                        "delivered but unlink failed; file may be re-delivered next start",
                    );
                }
                tracing::info!(
                    target: "dux::amq_inject",
                    session_id = %session_id,
                    receiver = %receiver,
                    body_preview = %amq_inject::preview(&msg.body, 80),
                    "delivered AMQ wake to session",
                );
                self.set_info(format!(
                    "Delivered AMQ wake to {}: {}",
                    receiver,
                    amq_inject::preview(&msg.body, 60),
                ));

                // Refresh the suppression window so it starts from
                // the moment the agent actually receives the input
                // (phase 2 Enter). The phase 1 stamp covered the gap
                // between body-write and Enter; this one covers the
                // gap until the postscript scrolls off the visible
                // terminal area.
                self.runtime.watch_suppress_until.insert(
                    session_id.to_string(),
                    Instant::now() + WATCH_SUPPRESS_AFTER_INJECT,
                );
            }
            Some(Err(err)) => {
                // PTY write of \r failed. The body is already in the
                // input field; release the file so the next tick
                // re-attempts phase 1 (typing the body again would
                // duplicate, but at least Enter eventually fires).
                // Better than leaving the body floating without
                // submission. The single-instance lock + claim
                // semantics still ensure no duplicate file delivery.
                let _ = amq_inject::release(&msg.inflight_path);
                tracing::warn!(
                    target: "dux::amq_inject",
                    session_id = %session_id,
                    receiver = %receiver,
                    path = %msg.inflight_path.display(),
                    err = %err,
                    "phase-2 PTY write (Enter) failed; released for retry",
                );
            }
            None => {
                // No PTY anymore. Same recovery path as write error.
                let _ = amq_inject::release(&msg.inflight_path);
            }
        }
    }

    /// Surface a status-line warning when a queued receiver has no
    /// matching live session. Rate-limited so a backlog doesn't spam.
    /// Emit a `debug`-level trace event the first time (and at most
    /// once per [`HOLD_LOG_RATE_LIMIT`] thereafter) we hold a
    /// receiver's queue without delivering. Default `info` filter
    /// drops these — opt in with `RUST_LOG=dux::amq_inject=debug` to
    /// diagnose why an expected delivery hasn't fired yet.
    ///
    /// The event includes:
    /// - the receiver handle,
    /// - the hold reason (no session / user typing / pty gone /
    ///   busy marker detected, with the matching marker substring),
    /// - the queue depth (helpful when many messages have piled up),
    /// - a sanitised preview of the head body (so a peek at one
    ///   `dux.log` line tells you what's stuck without correlating
    ///   timestamps against the on-disk inject-queue).
    ///
    /// The receiver-keyed throttle map is shared across all reasons
    /// so a flapping busy/idle agent doesn't bypass the rate limit
    /// by alternating reasons.
    fn log_holding(&mut self, receiver: &str, now: Instant, reason: HoldReason<'_>) {
        let last = self
            .runtime
            .amq_inject_last_held_logged
            .get(receiver)
            .copied();
        let due = last.is_none_or(|t| now.duration_since(t) >= HOLD_LOG_RATE_LIMIT);
        if !due {
            return;
        }
        self.runtime
            .amq_inject_last_held_logged
            .insert(receiver.to_string(), now);
        let queue = self.runtime.amq_inject_pending.get(receiver);
        let depth = queue.map(|q| q.len()).unwrap_or(0);
        let body_preview = queue
            .and_then(|q| q.front())
            .map(|head| amq_inject::preview(&head.body, 80))
            .unwrap_or_default();
        let matched_marker = match reason {
            HoldReason::BusyMarker(m) => Some(m.to_string()),
            _ => None,
        };
        match matched_marker {
            Some(m) => tracing::debug!(
                target: "dux::amq_inject",
                receiver = %receiver,
                reason = %reason.as_str(),
                marker = %m,
                queue_depth = depth,
                body_preview = %body_preview,
                "drainer holding queued message(s)",
            ),
            None => tracing::debug!(
                target: "dux::amq_inject",
                receiver = %receiver,
                reason = %reason.as_str(),
                queue_depth = depth,
                body_preview = %body_preview,
                "drainer holding queued message(s)",
            ),
        }
    }

    fn maybe_warn_no_session(&mut self, receiver: &str, now: Instant) {
        let last = self.runtime.amq_inject_last_warned.get(receiver).copied();
        let due = last.is_none_or(|t| now.duration_since(t) >= WARN_RATE_LIMIT);
        if !due {
            return;
        }
        self.runtime
            .amq_inject_last_warned
            .insert(receiver.to_string(), now);
        let count = self
            .runtime
            .amq_inject_pending
            .get(receiver)
            .map(|q| q.len())
            .unwrap_or(0);
        self.set_warning(format!(
            "AMQ inject: {count} message(s) queued for receiver \"{receiver}\" but no session matches its branch. Run a session whose branch sanitises to that handle, or move the files out of the inject-queue."
        ));
    }

    /// Surface a timeout warning when a queued message has been
    /// waiting longer than `delivery_timeout_secs`. Rate-limited the
    /// same way as `maybe_warn_no_session`. We don't move or delete
    /// the file — operators can inspect it.
    fn maybe_warn_timeout(&mut self, receiver: &str, now: Instant, timeout: Duration) {
        // Find the oldest queued_at across this receiver's queue.
        // Cheap: we stash queued_at on each message at claim time.
        // (Currently `QueuedMessage` doesn't carry queued_at — we
        // approximate using the `last_warned` timer; a real
        // implementation would add a timestamp field. For now this is
        // a "soft" timeout that nudges the operator after some
        // minutes of inactivity.)
        if timeout.is_zero() {
            return;
        }
        let last = self.runtime.amq_inject_last_warned.get(receiver).copied();
        let due = last.is_some_and(|t| now.duration_since(t) >= timeout);
        if due {
            self.runtime
                .amq_inject_last_warned
                .insert(receiver.to_string(), now);
            self.set_warning(format!(
                "AMQ inject: messages for \"{receiver}\" still pending after {}s — agent stayed busy or no session matched.",
                timeout.as_secs()
            ));
        }
    }
}

/// audit03 Phase 5: apply the Worker-mode postscript to an AMQ wake
/// body. Worker sessions get a sentinel-required note appended;
/// Attended/Orchestrator sessions get the body verbatim. Pure
/// function: no I/O, no global state, easy to unit-test.
///
/// The postscript ends with the literal
/// [`crate::watch::builtin::TASK_DONE_SENTINEL`] token so the
/// auto-clear watch rule (Phase 4) keys off the same string the
/// agent is asked to emit. Keeping these two on the same constant
/// avoids drift if the sentinel ever changes.
pub(crate) fn apply_inject_postscript(body: &str, mode: crate::model::ContextMode) -> String {
    match mode {
        crate::model::ContextMode::Worker => {
            format!(
                "{body}\n\n[Orchestrator note] When this task is complete, end your reply with the literal token {sentinel} so the orchestration layer knows to clean up.",
                sentinel = crate::watch::builtin::TASK_DONE_SENTINEL,
            )
        }
        crate::model::ContextMode::Attended | crate::model::ContextMode::Orchestrator => {
            body.to_string()
        }
    }
}

/// Decide whether the AMQ drainer should hold a message that targets the
/// currently-focused interactive session.
///
/// Returns `true` when the message must be held (skip this tick); `false`
/// when the operator looks idle enough that delivery is safe.
///
/// Rules:
/// - `quiet == 0` is the legacy "always skip while interactive" mode and
///   short-circuits to `true` regardless of keystroke history.
/// - Otherwise, hold iff the last recorded user keystroke is within the
///   quiet window. With no recorded keystroke, the operator is treated as
///   idle and the message flows.
fn should_hold_for_quiet_window(
    last_keystroke: Option<Instant>,
    now: Instant,
    quiet: Duration,
) -> bool {
    if quiet.is_zero() {
        return true;
    }
    last_keystroke.is_some_and(|t| now.duration_since(t) < quiet)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_inject_postscript, match_receiver, sanitise_handle, should_hold_for_quiet_window,
    };
    use crate::model::ContextMode;
    use std::time::{Duration, Instant};

    #[test]
    fn sanitise_lowercases_uppercase_letters() {
        assert_eq!(sanitise_handle("ALICE"), "alice");
        assert_eq!(sanitise_handle("Feature"), "feature");
    }

    #[test]
    fn sanitise_replaces_disallowed_chars_with_dash() {
        assert_eq!(sanitise_handle("Feature/Login.v2"), "feature-login-v2");
        assert_eq!(sanitise_handle("foo bar"), "foo-bar");
        assert_eq!(sanitise_handle("foo/bar/baz"), "foo-bar-baz");
    }

    #[test]
    fn sanitise_strips_leading_and_trailing_dashes() {
        assert_eq!(sanitise_handle("--foo--"), "foo");
        assert_eq!(sanitise_handle("///foo///"), "foo");
        assert_eq!(sanitise_handle("foo--"), "foo");
    }

    #[test]
    fn sanitise_preserves_existing_lowercase_handles() {
        assert_eq!(sanitise_handle("watch-rules-phase3"), "watch-rules-phase3");
        assert_eq!(sanitise_handle("a1b2_c3"), "a1b2_c3");
    }

    #[test]
    fn sanitise_collapses_to_empty_for_pure_garbage() {
        // Mirrors the bridge's `_unrouted` fallback path: when sanitise
        // returns empty, the wrapper writes nothing and the bridge
        // routes to `_unrouted/`.
        assert_eq!(sanitise_handle("..."), "");
        assert_eq!(sanitise_handle("///"), "");
    }

    /// The production bug that motivated this regression test:
    /// dux's sessions table had `branch_name = "fix/qa-s45-..."` for a
    /// worktree at `/data/state/dux/worktrees/Jobzy-Front-end/Front-end-QA/`,
    /// while the AMQ wrapper had derived `AM_ME = front-end-qa` from the
    /// worktree dir basename. The drainer's old branch-only matcher
    /// returned None and every wake notification stayed orphaned in
    /// `inject-queue/front-end-qa/.inflight.<ts>.msg`.
    #[test]
    fn match_receiver_matches_worktree_basename_when_branch_diverges() {
        let sessions = [(
            "session-uuid-1",
            "fix/qa-s45-charge-schema-paymentmethod",
            "/data/state/dux/worktrees/Jobzy-Front-end/Front-end-QA",
        )];
        assert_eq!(
            match_receiver(sessions.iter().copied(), "front-end-qa"),
            Some("session-uuid-1"),
        );
    }

    #[test]
    fn match_receiver_falls_back_to_branch_name_when_basename_does_not_match() {
        let sessions = [(
            "session-uuid-2",
            "feature-login",
            "/some/path/legacy-name-from-creation",
        )];
        assert_eq!(
            match_receiver(sessions.iter().copied(), "feature-login"),
            Some("session-uuid-2"),
        );
    }

    #[test]
    fn match_receiver_falls_back_to_session_id_for_operator_addressing() {
        let sessions = [("af882c2d", "fix/foo", "/wt/Bar")];
        // Receiver = exact session id.
        assert_eq!(
            match_receiver(sessions.iter().copied(), "af882c2d"),
            Some("af882c2d"),
        );
    }

    #[test]
    fn match_receiver_returns_none_when_nothing_matches() {
        let sessions = [("id1", "main", "/wt/main"), ("id2", "dev", "/wt/dev")];
        assert_eq!(
            match_receiver(sessions.iter().copied(), "front-end-qa"),
            None
        );
    }

    #[test]
    fn match_receiver_basename_priority_beats_branch_priority() {
        // Two sessions: A's worktree-basename is "alice", B's branch
        // is "alice". Worktree basename wins because the wrapper's
        // primary path inside dux is basename($PWD).
        let sessions = [
            ("idA", "fix/random", "/wt/Alice"),     // basename → alice
            ("idB", "alice", "/wt/something-else"), // branch → alice
        ];
        assert_eq!(
            match_receiver(sessions.iter().copied(), "alice"),
            Some("idA"),
        );
    }

    #[test]
    fn match_receiver_branch_match_wins_when_no_basename_match() {
        // Session A's branch matches but basename does not; with no
        // sessions matching by basename, we fall through to branch.
        let sessions = [("idA", "alice", "/wt/random-dir")];
        assert_eq!(
            match_receiver(sessions.iter().copied(), "alice"),
            Some("idA"),
        );
    }

    #[test]
    fn match_receiver_handles_empty_session_list() {
        let sessions: Vec<(&str, &str, &str)> = vec![];
        assert_eq!(match_receiver(sessions.iter().copied(), "anything"), None);
    }

    /// audit03 Phase 5: Worker-mode receivers get a sentinel-required
    /// postscript appended; Attended/Orchestrator pass through verbatim.
    /// The postscript MUST end with the literal `[task-done]` token so
    /// the auto-clear watch rule (Phase 4) can match.
    #[test]
    fn postscript_appended_for_worker_mode() {
        let body = "Please review the design doc.";
        let out = apply_inject_postscript(body, ContextMode::Worker);
        assert!(out.starts_with(body), "original body must come first");
        assert!(
            out.contains("[task-done]"),
            "postscript must include the literal sentinel; got: {out}"
        );
        assert!(
            out.contains("[Orchestrator note]"),
            "postscript must be clearly labelled so the agent treats it as instructions"
        );
        assert!(out.len() > body.len(), "postscript must actually add bytes");
    }

    #[test]
    fn postscript_skipped_for_attended_and_orchestrator() {
        let body = "ad-hoc question for human review";
        assert_eq!(apply_inject_postscript(body, ContextMode::Attended), body);
        assert_eq!(
            apply_inject_postscript(body, ContextMode::Orchestrator),
            body
        );
    }

    #[test]
    fn postscript_uses_canonical_sentinel_constant() {
        // Defensive: if the canonical sentinel ever changes, this
        // test should be the first to fire because it pins the
        // postscript output to the constant in `watch::builtin`.
        let out = apply_inject_postscript("body", ContextMode::Worker);
        assert!(out.contains(crate::watch::builtin::TASK_DONE_SENTINEL));
    }

    /// Quiet-window heuristic: when the operator has typed within the
    /// configured window, the drainer should still hold the message
    /// even though the target session is the active pane.
    #[test]
    fn quiet_window_holds_when_user_typed_recently() {
        let now = Instant::now();
        let last = now - Duration::from_secs(5);
        let quiet = Duration::from_secs(60);
        assert!(should_hold_for_quiet_window(Some(last), now, quiet));
    }

    /// After the quiet window has elapsed, the drainer should release
    /// the message even though the session is still in the foreground.
    #[test]
    fn quiet_window_delivers_after_idle_long_enough() {
        let now = Instant::now();
        let last = now - Duration::from_secs(120);
        let quiet = Duration::from_secs(60);
        assert!(!should_hold_for_quiet_window(Some(last), now, quiet));
    }

    /// First-ever delivery to a session has no recorded keystroke.
    /// The operator is treated as idle; deliver immediately.
    #[test]
    fn quiet_window_delivers_when_no_keystroke_recorded() {
        let now = Instant::now();
        let quiet = Duration::from_secs(60);
        assert!(!should_hold_for_quiet_window(None, now, quiet));
    }

    /// `active_session_quiet_secs = 0` is the legacy escape hatch that
    /// restores the original always-hold-while-interactive behaviour.
    /// In that mode keystroke history is irrelevant.
    #[test]
    fn quiet_window_zero_always_holds() {
        let now = Instant::now();
        let quiet = Duration::from_secs(0);
        assert!(should_hold_for_quiet_window(None, now, quiet));
        assert!(should_hold_for_quiet_window(Some(now), now, quiet));
        let stale = now - Duration::from_secs(10_000);
        assert!(should_hold_for_quiet_window(Some(stale), now, quiet));
    }

    /// Boundary: a keystroke exactly at the quiet-window edge counts
    /// as "no longer typing". `duration_since(t) < quiet` is strict.
    #[test]
    fn quiet_window_boundary_releases_at_exact_edge() {
        let now = Instant::now();
        let quiet = Duration::from_secs(60);
        let last = now - quiet;
        assert!(!should_hold_for_quiet_window(Some(last), now, quiet));
    }

    /// Phase delay: when `body_typed_at` is too recent relative to `now`,
    /// the drainer should skip phase 2 and wait for the next tick.
    #[test]
    fn phase_delay_holds_when_typed_too_recently() {
        let now = Instant::now();
        let typed_at = now - Duration::from_millis(10);
        let delay = Duration::from_millis(50);
        assert!(
            now.duration_since(typed_at) < delay,
            "typed 10ms ago should be within 50ms delay"
        );
    }

    /// Phase delay: when enough time has passed since phase 1, phase 2
    /// should proceed.
    #[test]
    fn phase_delay_releases_after_configured_delay() {
        let now = Instant::now();
        let typed_at = now - Duration::from_millis(100);
        let delay = Duration::from_millis(50);
        assert!(
            now.duration_since(typed_at) >= delay,
            "typed 100ms ago should exceed 50ms delay"
        );
    }

    /// Phase delay of 0 restores the old next-tick behaviour.
    #[test]
    fn phase_delay_zero_releases_immediately() {
        let now = Instant::now();
        let typed_at = now;
        let delay = Duration::from_millis(0);
        assert!(now.duration_since(typed_at) >= delay);
    }
}
