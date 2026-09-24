//! `[limits]` runtime guards (port of fork P1-AA and its #13 softening).
//!
//! Three things, all driven from [`crate::config::LimitsConfig`]:
//!
//! - the agent-creation gate: a hard refusal at `max_panes` (off by default) or
//!   when the disk is past `disk_high_water_pct`, and otherwise a soft warning
//!   once `max_panes_soft_warn` agents are live;
//! - a disk watchdog that samples the config directory's filesystem once a
//!   minute and surfaces a warning or error as it crosses the thresholds;
//! - an opt-in scrollback watchdog that stops the least recently active agent
//!   while the estimated scrollback total is over budget.
//!
//! The workers only sample and tick; every decision is made on the engine
//! thread from the event, so a reload's new thresholds apply at the next tick.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use super::Engine;
use super::events::{EventReaction, StatusUpdate};
use super::spawn_worker::{LoopControl, LoopWorkerSpec};
use crate::worker::WorkerEvent;

/// How often the disk watchdog samples. Long enough that the syscall is noise,
/// short enough to catch a runaway worktree before the host fills.
pub const DISK_WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);

/// How often the scrollback watchdog asks the engine to re-estimate.
pub const SCROLLBACK_WATCHDOG_INTERVAL: Duration = Duration::from_secs(60);

/// Rough bytes per scrollback cell (glyph plus style) in the terminal grid.
/// The watchdog only needs "over the budget or not", so an estimate beats a
/// per-cell walk every minute.
const BYTES_PER_CELL: usize = 4;

/// The engine's `[limits]` state.
#[derive(Debug, Default)]
pub struct LimitsRuntime {
    /// Latest disk usage sample, `None` until the first one lands. The spawn
    /// gate reads it, so a nearly full disk refuses agents between samples too.
    pub disk_usage_pct: Option<u8>,
    /// Whether the last disk status this module posted was a warning or error,
    /// so dropping back under both thresholds posts one all-clear rather than
    /// staying silent while a stale warning is on screen.
    disk_alert_shown: bool,
    disk_watchdog_started: Arc<AtomicBool>,
    scrollback_watchdog_started: Arc<AtomicBool>,
}

impl Engine {
    /// Agents with at least one live provider, the count every pane limit is
    /// measured in (an agent with several tabs is still one agent).
    pub fn live_agent_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|session| self.any_tab_active(&session.id))
            .count()
    }

    /// The hard `[limits]` refusal for a new agent, if any.
    pub fn refuse_agent_spawn_for_limits(&self) -> Option<String> {
        self.config
            .limits
            .refuse_agent_spawn(self.live_agent_count(), self.limits.disk_usage_pct)
    }

    /// The soft `[limits]` warning for a new agent, if any. Independent of the
    /// hard refusal: the caller checks that first.
    pub fn soft_warn_for_pane_count(&self) -> Option<String> {
        self.config
            .limits
            .soft_warn_for_pane_count(self.live_agent_count())
    }

    /// The hard `[limits]` refusal for a new companion terminal, if any.
    pub fn refuse_companion_terminal_for_limits(&self) -> Option<String> {
        self.config
            .limits
            .refuse_companion_terminal(self.companion_terminals.len())
    }

    /// Start the `[limits]` watchdogs. Idempotent, like the other global
    /// pollers, because the surface flip calls it again on the same engine.
    /// The scrollback watchdog starts only when auto-detach is on; a reload
    /// that turns it on later starts it through [`Self::retune_limits`].
    pub fn spawn_limits_watchdogs(&self) {
        self.spawn_disk_watchdog();
        if self.config.limits.enable_scrollback_overflow_autodetach {
            self.spawn_scrollback_watchdog();
        }
    }

    /// Apply a reloaded `[limits]`: start the scrollback watchdog if the reload
    /// turned it on. Thresholds need nothing, they are read per event.
    pub fn retune_limits(&self) {
        if self.config.limits.enable_scrollback_overflow_autodetach {
            self.spawn_scrollback_watchdog();
        }
    }

    fn spawn_disk_watchdog(&self) {
        if self
            .limits
            .disk_watchdog_started
            .swap(true, Ordering::Relaxed)
        {
            return;
        }
        let root = self.paths.root.clone();
        let mut first = true;
        let started = self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "disk-watchdog".into(),
                feature: "the [limits] disk usage checks".into(),
                remedy: crate::poller_status::REMEDY_RESTART_DUX.into(),
            },
            move |tx| {
                // The first sample goes out at once, so a disk that is already
                // full refuses agents from boot rather than a minute later.
                if !std::mem::take(&mut first) {
                    thread::sleep(DISK_WATCHDOG_INTERVAL);
                }
                if let Some(pct) = crate::config::sample_disk_usage_pct(&root)
                    && tx.send(WorkerEvent::DiskUsageSampled(pct)).is_err()
                {
                    return LoopControl::Break;
                }
                LoopControl::Continue
            },
        );
        if !started {
            self.limits
                .disk_watchdog_started
                .store(false, Ordering::Relaxed);
        }
    }

    fn spawn_scrollback_watchdog(&self) {
        if self
            .limits
            .scrollback_watchdog_started
            .swap(true, Ordering::Relaxed)
        {
            return;
        }
        let started = self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "scrollback-watchdog".into(),
                feature: "the [limits] scrollback budget".into(),
                remedy: crate::poller_status::REMEDY_RESTART_DUX.into(),
            },
            move |tx| {
                thread::sleep(SCROLLBACK_WATCHDOG_INTERVAL);
                // The grids live on the engine thread, so the worker only
                // ticks and the engine does the estimate.
                if tx.send(WorkerEvent::ScrollbackWatchdogTick).is_err() {
                    return LoopControl::Break;
                }
                LoopControl::Continue
            },
        );
        if !started {
            self.limits
                .scrollback_watchdog_started
                .store(false, Ordering::Relaxed);
        }
    }

    /// Record a disk sample and say what the user should see.
    pub fn handle_disk_usage_event(&mut self, pct: u8) -> EventReaction {
        self.limits.disk_usage_pct = Some(pct);
        match self.config.limits.disk_usage_status(pct) {
            Some((is_error, message)) => {
                self.limits.disk_alert_shown = true;
                EventReaction::Status(if is_error {
                    StatusUpdate::error(message)
                } else {
                    StatusUpdate::warning(message)
                })
            }
            None if std::mem::take(&mut self.limits.disk_alert_shown) => {
                EventReaction::Status(StatusUpdate::info(format!(
                    "The disk holding the dux config directory is back to {pct}% full."
                )))
            }
            None => EventReaction::Nothing,
        }
    }

    /// Estimated scrollback memory of every live provider, from each one's
    /// ring capacity and width.
    pub fn estimate_total_scrollback_bytes(&self) -> usize {
        self.providers
            .values()
            .filter(|client| client.is_live())
            .map(|client| {
                let cols = client.grid_size().map_or(80, |(_, cols)| usize::from(cols));
                client
                    .scrollback_capacity()
                    .saturating_mul(cols)
                    .saturating_mul(BYTES_PER_CELL)
            })
            .fold(0usize, usize::saturating_add)
    }

    /// While the estimate is over budget, stop the live agent that was active
    /// least recently. Stopping goes through the same kill every surface uses,
    /// so the agent is Detached (not deleted) and can be resumed.
    pub fn handle_scrollback_watchdog_tick(&mut self) -> EventReaction {
        let Some(budget) = self.config.limits.scrollback_budget_bytes() else {
            return EventReaction::Nothing;
        };
        let mut reactions = Vec::new();
        while self.estimate_total_scrollback_bytes() > budget {
            let Some(victim) = self
                .sessions
                .iter()
                .filter(|session| self.any_tab_active(&session.id))
                .min_by_key(|session| session.updated_at)
                .map(|session| session.id.clone())
            else {
                break;
            };
            let mut killed_any = false;
            for tab_id in self.tab_ids_for_session(&victim) {
                killed_any |= self.kill_tab_runtime(tab_id.as_str()).killed;
            }
            if !killed_any {
                break;
            }
            crate::logger::info(&format!(
                "scrollback watchdog stopped agent {victim}: estimated scrollback \
                 over limits.max_total_scrollback_mb = {} MiB",
                self.config.limits.max_total_scrollback_mb
            ));
            let name = self
                .session_by_id(&victim)
                .and_then(|session| {
                    session
                        .title
                        .clone()
                        .or_else(|| session.branch_name().map(str::to_string))
                })
                .unwrap_or(victim);
            reactions.push(EventReaction::Status(StatusUpdate::warning(format!(
                "Stopped agent \"{name}\": total scrollback went over \
                 limits.max_total_scrollback_mb = {} MiB. Its record is kept and it \
                 can be resumed.",
                self.config.limits.max_total_scrollback_mb
            ))));
        }
        if !reactions.is_empty() {
            self.sync_has_active_processes();
            reactions.push(EventReaction::RebuildLeftItems);
        }
        match reactions.len() {
            0 => EventReaction::Nothing,
            _ => EventReaction::Multi(reactions),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};
    use crate::pty::PtyClient;
    use crate::statusline::StatusTone;

    fn go_live(engine: &mut Engine, session_id: &str, scrollback: usize) {
        let client =
            PtyClient::spawn("cat", &[], Path::new("."), 10, 40, scrollback).expect("spawn cat");
        let tab = engine
            .session_by_id(session_id)
            .expect("session")
            .slot_tab_id()
            .to_owned();
        engine.providers.insert(tab, client);
    }

    fn engine_with(sessions: &[&str]) -> (Engine, tempfile::TempDir) {
        let (mut engine, tmp) = test_engine();
        for id in sessions {
            engine
                .sessions
                .push(sample_session(id, "p1", &format!("feat/{id}")));
        }
        (engine, tmp)
    }

    fn status_of(reaction: &EventReaction) -> Option<(StatusTone, String)> {
        match reaction {
            EventReaction::Status(update) => Some((update.tone, update.message.clone())),
            _ => None,
        }
    }

    #[test]
    fn refuse_agent_spawn_when_max_panes_reached() {
        let (mut engine, _tmp) = engine_with(&["s1", "s2"]);
        go_live(&mut engine, "s1", 100);
        go_live(&mut engine, "s2", 100);
        engine.config.limits.max_panes = 2;

        let reason = engine
            .refuse_agent_spawn_for_limits()
            .expect("must refuse at the cap");
        assert!(reason.contains("max_panes"), "names the knob: {reason}");
        assert!(reason.contains("Detach"), "names a way out: {reason}");

        // An agent that is no longer live does not count.
        engine.kill_tab_runtime("s2-slot");
        assert!(engine.refuse_agent_spawn_for_limits().is_none());

        // 0 means no cap.
        go_live(&mut engine, "s2", 100);
        engine.config.limits.max_panes = 0;
        assert!(
            engine.refuse_agent_spawn_for_limits().is_none(),
            "max_panes = 0 must mean unlimited"
        );
    }

    #[test]
    fn soft_warn_for_pane_count_fires_at_threshold_only() {
        let (mut engine, _tmp) = engine_with(&["s1", "s2"]);
        engine.config.limits.max_panes = 0;
        engine.config.limits.max_panes_soft_warn = 2;

        assert!(engine.soft_warn_for_pane_count().is_none());
        go_live(&mut engine, "s1", 100);
        assert!(engine.soft_warn_for_pane_count().is_none());
        go_live(&mut engine, "s2", 100);
        let warn = engine
            .soft_warn_for_pane_count()
            .expect("must warn at the threshold");
        assert!(
            warn.contains("max_panes_soft_warn"),
            "names the knob: {warn}"
        );
        assert!(warn.contains('2'), "reports the count: {warn}");

        engine.config.limits.max_panes_soft_warn = 0;
        assert!(
            engine.soft_warn_for_pane_count().is_none(),
            "0 must silence the warning"
        );
    }

    #[test]
    fn soft_warn_independent_of_hard_cap() {
        let (mut engine, _tmp) = engine_with(&["s1"]);
        engine.config.limits.max_panes = 5;
        engine.config.limits.max_panes_soft_warn = 1;
        go_live(&mut engine, "s1", 100);
        assert!(engine.refuse_agent_spawn_for_limits().is_none());
        assert!(engine.soft_warn_for_pane_count().is_some());
    }

    #[test]
    fn refuse_agent_spawn_when_disk_above_high_water() {
        let (mut engine, _tmp) = engine_with(&["s1"]);
        engine.config.limits.disk_high_water_pct = 95;

        // Before the first sample the gate must not fire.
        assert!(engine.refuse_agent_spawn_for_limits().is_none());

        engine.limits.disk_usage_pct = Some(94);
        assert!(engine.refuse_agent_spawn_for_limits().is_none());

        engine.limits.disk_usage_pct = Some(96);
        let reason = engine
            .refuse_agent_spawn_for_limits()
            .expect("must refuse above high water");
        assert!(reason.contains("96%"), "reports the pct: {reason}");
        assert!(
            reason.contains("disk_high_water_pct"),
            "names the knob: {reason}"
        );
        assert!(
            reason.contains("extend the volume"),
            "names a way out: {reason}"
        );

        engine.config.limits.disk_high_water_pct = 0;
        assert!(
            engine.refuse_agent_spawn_for_limits().is_none(),
            "0 turns the disk refusal off"
        );
    }

    #[test]
    fn dispatch_create_agent_request_is_refused_at_the_cap_and_warns_under_it() {
        let (mut engine, _tmp) = engine_with(&["s1"]);
        go_live(&mut engine, "s1", 100);
        let request = || crate::worker::CreateAgentRequest::NewProject {
            project: crate::engine::test_support::sample_project("p1", "/nonexistent/p1"),
            custom_name: None,
            use_existing_branch: false,
            pull_before_create: false,
            copy_uncommitted_changes: false,
        };

        engine.config.limits.max_panes = 1;
        let reaction = engine
            .apply(crate::engine::Command::DispatchCreateAgentRequest {
                request: Box::new(request()),
                busy_message: "Creating".into(),
                term_size: (24, 80),
            })
            .expect("apply");
        let (tone, message) = status_of(&reaction).expect("a refusal status");
        assert_eq!(tone, StatusTone::Error);
        assert!(message.contains("max_panes"), "{message}");
        assert!(
            !engine.is_in_flight(&crate::engine::InFlightKey::CreateAgent),
            "a refused create must not start"
        );

        engine.config.limits.max_panes = 0;
        engine.config.limits.max_panes_soft_warn = 1;
        let reaction = engine
            .apply(crate::engine::Command::DispatchCreateAgentRequest {
                request: Box::new(request()),
                busy_message: "Creating".into(),
                term_size: (24, 80),
            })
            .expect("apply");
        let EventReaction::Multi(items) = reaction else {
            panic!("expected the warning beside the create's own reaction");
        };
        let (tone, message) = status_of(&items[0]).expect("the warning first");
        assert_eq!(tone, StatusTone::Warning);
        assert!(message.contains("max_panes_soft_warn"), "{message}");
        assert!(
            engine.is_in_flight(&crate::engine::InFlightKey::CreateAgent),
            "a soft warning must not stop the create"
        );
    }

    #[test]
    fn companion_terminal_cap_refuses_at_the_limit() {
        let (mut engine, _tmp) = engine_with(&[]);
        assert!(engine.refuse_companion_terminal_for_limits().is_none());
        engine.config.limits.max_companion_terminals = 1;
        engine
            .create_standalone_terminal(10, 40)
            .expect("first terminal");
        let reason = engine
            .refuse_companion_terminal_for_limits()
            .expect("refused at the cap");
        assert!(reason.contains("max_companion_terminals"), "{reason}");
        let err = engine
            .create_standalone_terminal(10, 40)
            .expect_err("the second terminal is refused");
        assert!(err.to_string().contains("max_companion_terminals"));
    }

    #[test]
    fn disk_usage_event_warns_errors_and_clears() {
        let (mut engine, _tmp) = engine_with(&[]);
        assert!(matches!(
            engine.handle_disk_usage_event(50),
            EventReaction::Nothing
        ));
        assert_eq!(engine.limits.disk_usage_pct, Some(50));

        let (tone, message) = status_of(&engine.handle_disk_usage_event(85)).unwrap();
        assert_eq!(tone, StatusTone::Warning);
        assert!(message.contains("disk_warn_pct"), "{message}");

        let (tone, message) = status_of(&engine.handle_disk_usage_event(97)).unwrap();
        assert_eq!(tone, StatusTone::Error);
        assert!(message.contains("disk_high_water_pct"), "{message}");

        let (tone, _) = status_of(&engine.handle_disk_usage_event(40)).unwrap();
        assert_eq!(tone, StatusTone::Info, "one all-clear after an alert");
        assert!(matches!(
            engine.handle_disk_usage_event(41),
            EventReaction::Nothing
        ));
    }

    #[test]
    fn scrollback_auto_detach_picks_oldest_session() {
        let (mut engine, _tmp) = engine_with(&["old", "new"]);
        let now = chrono::Utc::now();
        engine.sessions[0].updated_at = now - chrono::Duration::hours(2);
        engine.sessions[1].updated_at = now;
        // 40 cols * 4 bytes * 10_000 lines = 1.6 MB each.
        go_live(&mut engine, "old", 10_000);
        go_live(&mut engine, "new", 10_000);
        engine.config.limits.enable_scrollback_overflow_autodetach = true;
        engine.config.limits.max_total_scrollback_mb = 2;

        let reaction = engine.handle_scrollback_watchdog_tick();
        assert!(!engine.any_tab_active("old"), "the oldest agent is stopped");
        assert!(engine.any_tab_active("new"), "one stop was enough");
        assert!(matches!(reaction, EventReaction::Multi(_)));
        assert_eq!(
            engine.session_by_id("old").unwrap().status,
            crate::model::SessionStatus::Detached,
            "stopped, not deleted"
        );
    }

    #[test]
    fn scrollback_watchdog_is_a_noop_when_auto_detach_is_off() {
        let (mut engine, _tmp) = engine_with(&["s1"]);
        go_live(&mut engine, "s1", 10_000);
        engine.config.limits.max_total_scrollback_mb = 1;
        engine.config.limits.enable_scrollback_overflow_autodetach = false;
        assert!(matches!(
            engine.handle_scrollback_watchdog_tick(),
            EventReaction::Nothing
        ));
        assert!(engine.any_tab_active("s1"));
    }

    #[test]
    fn disk_watchdog_sends_a_first_sample_at_once() {
        let (engine, _tmp) = engine_with(&[]);
        let started = Instant::now();
        engine.spawn_limits_watchdogs();
        // Idempotent: a second call must not start a second sampler.
        engine.spawn_limits_watchdogs();
        let event = engine
            .worker_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a first sample well before the interval");
        assert!(matches!(event, WorkerEvent::DiskUsageSampled(pct) if pct <= 100));
        assert!(started.elapsed() < DISK_WATCHDOG_INTERVAL);
        assert!(
            engine
                .worker_rx
                .recv_timeout(Duration::from_millis(300))
                .is_err(),
            "only one watchdog runs"
        );
    }
}
