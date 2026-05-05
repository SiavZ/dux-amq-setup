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

use crate::amq_inject::{self, QueuedMessage, UNROUTED_RECEIVER, snapshot_indicates_busy};

/// How long between repeat warnings about the same un-deliverable
/// receiver. Stops a queue full of messages for an unknown handle
/// from spamming the status line at tick rate.
const WARN_RATE_LIMIT: Duration = Duration::from_secs(60);

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
        let outcome = match amq_inject::scan_queue_dir(&queue_dir) {
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

        for (path, reason) in &outcome.rejections {
            tracing::warn!(
                target: "dux::amq_inject",
                path = %path.display(),
                reason = %reason.human(),
                "queue entry rejected",
            );
        }

        for pending in outcome.messages {
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
            match amq_inject::read_validated(&inflight, max_bytes) {
                Ok(body) => {
                    let queued = QueuedMessage {
                        receiver: pending.receiver.clone(),
                        body,
                        inflight_path: inflight,
                        source_path: pending.path.clone(),
                    };
                    self.runtime
                        .amq_inject_pending
                        .entry(pending.receiver.clone())
                        .or_default()
                        .push_back(queued);
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

        for receiver in receivers {
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
                continue;
            };

            // Don't interrupt the user mid-prompt. Same rule that
            // `tick_watch_engines` enforces on its own auto-actions.
            if active_session.as_deref() == Some(session_id.as_str()) {
                continue;
            }

            // Drain the head as long as the agent is idle. We loop
            // here so a backlog of messages drains in one tick when
            // the agent's been idle for a while. The busy check is
            // re-run between deliveries because each Enter we type
            // changes the snapshot.
            loop {
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
                        break;
                    }
                };
                if snapshot_indicates_busy(&snapshot, &busy_markers) {
                    self.maybe_warn_timeout(&receiver, now, timeout);
                    break;
                }

                // Pop head and deliver.
                let Some(head) = self
                    .runtime
                    .amq_inject_pending
                    .get_mut(&receiver)
                    .and_then(|q| q.pop_front())
                else {
                    break;
                };
                self.deliver_inject(&session_id, &receiver, &head);
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

    /// Type the body into the session's PTY and submit with `\r`.
    /// Reuses `crate::app::input::macro_payload_bytes` so embedded
    /// newlines become Alt-Enter (a newline within the prompt) rather
    /// than premature submits — the same chokepoint watch effects use.
    fn deliver_inject(&mut self, session_id: &str, receiver: &str, msg: &QueuedMessage) {
        let mut payload = crate::app::input::macro_payload_bytes(&msg.body);
        // Trailing CR submits the prompt. Equivalent to
        // `WatchEffect::SendText { append_enter: true }`.
        payload.push(b'\r');
        let write_result = self
            .find_pty_handle(session_id)
            .map(|handle| handle.write_bytes(&payload));
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
            }
            Some(Err(err)) => {
                // PTY write failed — most likely the agent process
                // exited between the busy check and the write. Put
                // the file back and try again next tick.
                let _ = amq_inject::release(&msg.inflight_path);
                tracing::warn!(
                    target: "dux::amq_inject",
                    session_id = %session_id,
                    receiver = %receiver,
                    path = %msg.inflight_path.display(),
                    err = %err,
                    "PTY write failed; released for retry",
                );
            }
            None => {
                // No PTY anymore. Same recovery path as a write error.
                let _ = amq_inject::release(&msg.inflight_path);
            }
        }
    }

    /// Surface a status-line warning when a queued receiver has no
    /// matching live session. Rate-limited so a backlog doesn't spam.
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

#[cfg(test)]
mod tests {
    use super::{match_receiver, sanitise_handle};

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
}
