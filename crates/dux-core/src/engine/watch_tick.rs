//! Engine half of the watch-rule feature: keep one [`WatchEngine`] per live
//! agent tab in step with config, feed it the tab's recent output every tick,
//! and carry out what it decides.
//!
//! Both surfaces call [`Engine::tick_watch_rules`] exactly once per tick (the
//! TUI run loop, or the web actor's maintenance sweep when no TUI is running),
//! the same contract as `poll_pty_activity`, so a rule never fires twice.

use std::time::{Duration, Instant};

use crate::ids::{TabId, TabIdRef};
use crate::watch::delivery::{
    MIN_ENTER_PHASE_DELAY_MS, effective_enter_phase_delay, inject_body_bytes_for_provider,
    submit_key_bytes_for_provider,
};
use crate::watch::runtime::AttachedWatch;
use crate::watch::{RuleSnapshot, WatchEffect, WatchEngine, WatchRule, WatchRuleKind};

use super::{Engine, StatusUpdate};

/// How many of the newest non-blank rows a rule is matched against. Enough
/// for an error block plus the prompt under it, small enough that an old
/// incident scrolls out and a new one reads as new.
pub const WATCH_SCAN_ROWS: usize = 30;

/// A tab the user typed into within this window is left alone: a rule must not
/// type into the middle of the user's own prompt. Short on purpose, because
/// rules react to terminal states (the fork capped its quiet window at 5s).
pub const WATCH_TYPING_QUIET: Duration = Duration::from_secs(5);

/// The per-session inputs the watch engine reads from the session settings
/// store (Worker mode + auto-clear opt-in, and manual arm overrides).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchSessionSettings {
    /// Worker mode AND auto-clear-on-task-done both on: attach the built-in
    /// auto-clear rule. Asymmetric default: off.
    pub auto_clear: bool,
    /// Manual arm (`true`) / disarm (`false`) overrides by rule index into the
    /// provider's `[[providers.<name>.watch]]` array, sorted by index.
    pub arm_overrides: Vec<(usize, bool)>,
}

/// One row of the watch-rules list: a rule on a live tab.
#[derive(Clone, Debug)]
pub struct WatchRuleRow {
    pub tab_id: TabId,
    pub session_id: String,
    pub snapshot: RuleSnapshot,
    /// The built-in auto-clear rule. Not user-toggleable from the list.
    pub built_in: bool,
}

/// What a tab's engine should be built from.
struct DesiredWatch {
    rules: Vec<WatchRule>,
    overrides: Vec<(usize, bool)>,
    auto_clear_idx: Option<usize>,
}

impl Engine {
    /// Settings the watch engine needs for `session_id`.
    /// Auto-clear needs BOTH Worker mode and the explicit opt-in (the
    /// operator opts in twice); a missing record is the safe default.
    pub fn watch_session_settings(&self, session_id: &str) -> WatchSessionSettings {
        let Some(s) = self.session_settings(session_id) else {
            return WatchSessionSettings::default();
        };
        let mut arm_overrides: Vec<(usize, bool)> =
            s.watch_rule_arm.iter().map(|(i, a)| (*i, *a)).collect();
        arm_overrides.sort_unstable();
        WatchSessionSettings {
            auto_clear: s.mode == crate::session_settings::ContextMode::Worker
                && s.auto_clear_on_task_done,
            arm_overrides,
        }
    }

    /// Whether the built-in auto-clear rule must hold off this tick: the agent
    /// is busy, AMQ mail is pending for it, or it collaborated recently. A
    /// suppressed tick rebaselines the rule, so a sentinel seen while held
    /// never fires later.
    ///
    /// The AMQ side owns the answer (busy markers, queued wakes, pending
    /// mail, the collaboration quiet window): see
    /// [`Engine::amq_blocks_auto_clear`].
    pub fn watch_auto_clear_suppressed(&self, _tab_id: &TabIdRef, session_id: &str) -> bool {
        self.amq_blocks_auto_clear(session_id)
    }

    /// Pause a tab's watch rules until `until`. An AMQ inject calls this after
    /// typing a postscript that may itself match a rule (the task-done
    /// sentinel); when the window ends the rules rebaseline once so that text
    /// never fires.
    pub fn suppress_watch_rules(&mut self, tab_id: &TabIdRef, until: Instant) {
        self.watch.suppress_until.insert(tab_id.to_owned(), until);
    }

    /// The rules a tab should be running right now, or `None` when it should
    /// have no engine.
    fn desired_watch(&self, tab_id: &TabIdRef) -> Option<DesiredWatch> {
        let session_id = self.owning_session_for_tab(tab_id.as_str())?;
        let session = self.session_by_id(&session_id)?;
        let provider = self.tab_running_provider(session, tab_id);
        let mut rules = self
            .config
            .providers
            .get(provider.as_str())
            .map(|cfg| cfg.watch.clone())
            .unwrap_or_default();
        let settings = self.watch_session_settings(&session_id);
        let auto_clear_idx = if settings.auto_clear {
            let clear = crate::watch::builtin::provider_clear_command(&provider);
            rules.push(crate::watch::builtin::auto_clear_rule_for(clear));
            Some(rules.len() - 1)
        } else {
            None
        };
        if rules.is_empty() {
            return None;
        }
        Some(DesiredWatch {
            rules,
            overrides: settings.arm_overrides,
            auto_clear_idx,
        })
    }

    /// Attach, rebuild or drop each live tab's engine so it matches config.
    /// An unchanged rule set keeps its engine (and so its attempt counts and
    /// cooldowns); a changed one is rebuilt from scratch.
    fn sync_watch_engines(&mut self, statuses: &mut Vec<StatusUpdate>) {
        self.watch
            .attached
            .retain(|tab, _| self.providers.contains_key(tab.as_ref_id()));
        let tabs: Vec<TabId> = self.providers.keys().cloned().collect();
        for tab in tabs {
            let desired = self.desired_watch(tab.as_ref_id());
            let Some(DesiredWatch {
                rules,
                overrides,
                auto_clear_idx,
            }) = desired
            else {
                self.watch.attached.remove(&tab);
                continue;
            };
            if let Some(current) = self.watch.attached.get(&tab)
                && current.rules == rules
                && current.arm_overrides == overrides
            {
                continue;
            }
            let (mut engine, errors) = WatchEngine::new(tab.as_str(), &rules);
            for err in errors {
                crate::logger::warn(&format!("[watch] tab {tab}: {err}"));
                statuses.push(StatusUpdate::warning(err));
            }
            for &(idx, armed) in &overrides {
                // A stale index (the config shrank) is ignored, and the
                // built-in rule's index is reserved against forged overrides.
                if idx >= engine.rule_count() || Some(idx) == auto_clear_idx {
                    continue;
                }
                if armed {
                    engine.rearm(idx);
                } else {
                    engine.disarm(idx);
                }
            }
            self.watch.attached.insert(
                tab,
                AttachedWatch {
                    engine,
                    rules,
                    arm_overrides: overrides,
                    auto_clear_idx,
                },
            );
        }
    }

    /// Drive every live tab's watch rules one tick and return the status lines
    /// to show. Call once per tick from exactly one surface.
    pub fn tick_watch_rules(&mut self) -> Vec<StatusUpdate> {
        let mut statuses = Vec::new();
        // Phase 2 of two-phase delivery first, so a submit key deferred last
        // tick lands before anything new is typed.
        self.flush_pending_watch_enters(Instant::now());
        self.sync_watch_engines(&mut statuses);
        if self.watch.attached.is_empty() {
            return statuses;
        }
        let now = Instant::now();
        let tabs: Vec<TabId> = self.watch.attached.keys().cloned().collect();
        for tab in tabs {
            let typed_recently = self
                .pty_input
                .get(tab.as_str())
                .is_some_and(|at| now.duration_since(*at) < WATCH_TYPING_QUIET);
            if typed_recently {
                continue;
            }
            let Some(snapshot) = self
                .providers
                .get(tab.as_ref_id())
                .map(|p| p.scan_recent_lines(WATCH_SCAN_ROWS))
            else {
                continue;
            };
            if let Some(until) = self.watch.suppress_until.get(&tab).copied() {
                if now < until {
                    continue;
                }
                self.watch.suppress_until.remove(&tab);
                if let Some(attached) = self.watch.attached.get_mut(&tab) {
                    attached.engine.rebaseline(&snapshot);
                }
            }
            let session_id = self
                .owning_session_for_tab(tab.as_str())
                .unwrap_or_default();
            let hold_auto_clear = self
                .watch
                .attached
                .get(&tab)
                .is_some_and(|a| a.auto_clear_idx.is_some())
                && self.watch_auto_clear_suppressed(tab.as_ref_id(), &session_id);
            let effects = match self.watch.attached.get_mut(&tab) {
                Some(attached) => {
                    if hold_auto_clear {
                        attached
                            .engine
                            .rebaseline_kind(&snapshot, WatchRuleKind::BuiltInAutoClear);
                    }
                    attached.engine.observe(&snapshot, now)
                }
                None => continue,
            };
            for effect in effects {
                self.apply_watch_effect(&tab, effect, now, &mut statuses);
            }
        }
        statuses
    }

    fn apply_watch_effect(
        &mut self,
        tab: &TabId,
        effect: WatchEffect,
        now: Instant,
        statuses: &mut Vec<StatusUpdate>,
    ) {
        match effect {
            WatchEffect::SendText { text, append_enter } => {
                // Phase 1: the body only. The submit key waits for
                // `flush_pending_watch_enters` so an Ink reader sees it as a
                // separate keystroke.
                let provider = self.watch_provider_for_tab(tab.as_ref_id());
                let payload = inject_body_bytes_for_provider(&text, provider.as_ref());
                let Some(client) = self.providers.get(tab.as_ref_id()) else {
                    return;
                };
                if client.write_bytes(&payload).is_err() {
                    return;
                }
                if append_enter {
                    self.watch.pending_enters.insert(tab.clone(), now);
                }
            }
            WatchEffect::StatusInfo(msg) => statuses.push(StatusUpdate::info(msg)),
            WatchEffect::StatusWarning(msg) => statuses.push(StatusUpdate::warning(msg)),
        }
    }

    fn watch_provider_for_tab(&self, tab: &TabIdRef) -> Option<crate::model::ProviderKind> {
        let session_id = self.owning_session_for_tab(tab.as_str())?;
        let session = self.session_by_id(&session_id)?;
        Some(self.tab_running_provider(session, tab))
    }

    /// Phase 2 of two-phase delivery: send the submit key to every tab whose
    /// body has waited at least the phase delay. A tab whose PTY went away
    /// drops its deferred Enter rather than retrying it later, when it could
    /// submit whatever the user has typed since.
    fn flush_pending_watch_enters(&mut self, now: Instant) {
        if self.watch.pending_enters.is_empty() {
            return;
        }
        let delay = effective_enter_phase_delay(MIN_ENTER_PHASE_DELAY_MS);
        let due: Vec<TabId> = self
            .watch
            .pending_enters
            .iter()
            .filter(|(_, typed_at)| now.duration_since(**typed_at) >= delay)
            .map(|(tab, _)| tab.clone())
            .collect();
        for tab in due {
            self.watch.pending_enters.remove(&tab);
            let provider = self.watch_provider_for_tab(tab.as_ref_id());
            let key = submit_key_bytes_for_provider(provider.as_ref());
            if let Some(client) = self.providers.get(tab.as_ref_id()) {
                let _ = client.write_bytes(key);
            }
        }
    }

    /// Every loaded rule on every live tab, ordered by session then rule.
    pub fn watch_rule_rows(&self) -> Vec<WatchRuleRow> {
        let mut rows = Vec::new();
        for (tab, attached) in &self.watch.attached {
            let session_id = self
                .owning_session_for_tab(tab.as_str())
                .unwrap_or_default();
            for snapshot in attached.engine.rules_snapshot() {
                rows.push(WatchRuleRow {
                    tab_id: tab.clone(),
                    session_id: session_id.clone(),
                    built_in: Some(snapshot.idx) == attached.auto_clear_idx,
                    snapshot,
                });
            }
        }
        rows.sort_by(|a, b| {
            (a.session_id.as_str(), a.tab_id.as_str(), a.snapshot.idx).cmp(&(
                b.session_id.as_str(),
                b.tab_id.as_str(),
                b.snapshot.idx,
            ))
        });
        rows
    }

    /// Disarm an armed rule or re-arm a disarmed one on a live tab. Returns
    /// the status line to show, or `None` when the tab or rule is gone.
    ///
    /// The new arm state is persisted into the session's `watch_rule_arm`,
    /// so a manual disarm survives restart (and the next sync, which rebuilds
    /// from the persisted overrides, agrees with what the user chose).
    pub fn toggle_watch_rule(&mut self, tab: &TabIdRef, idx: usize) -> Option<StatusUpdate> {
        let attached = self.watch.attached.get_mut(tab)?;
        if Some(idx) == attached.auto_clear_idx {
            return Some(StatusUpdate::info(
                "The auto-clear rule follows the session's Worker settings; change it there.",
            ));
        }
        let snapshot = attached.engine.rules_snapshot().into_iter().nth(idx)?;
        let disarmed = matches!(snapshot.state, crate::watch::RuleStateKind::Disarmed);
        let verb = if disarmed {
            attached.engine.rearm(idx);
            "re-armed"
        } else {
            attached.engine.disarm(idx);
            "disarmed"
        };
        if let Some(session_id) = self.owning_session_for_tab(tab.as_str()) {
            let mut settings = self.session_settings_or_default(&session_id);
            settings.watch_rule_arm.insert(idx, disarmed);
            if let Err(err) = self.persist_session_settings_only(&session_id, settings) {
                // The live engine keeps the toggle; its recorded overrides stay
                // matched to the (unchanged) saved settings, so the next sync
                // does not rebuild it and the toggle lasts for this run.
                return Some(StatusUpdate::warning(format!(
                    "Watch rule \"{}\" {verb} for this run, but saving it failed: {err}",
                    snapshot.label
                )));
            }
            // Saved: record the new overrides so the next sync sees the live
            // engine already matches the settings and keeps it.
            let saved = self.watch_session_settings(&session_id).arm_overrides;
            if let Some(attached) = self.watch.attached.get_mut(tab) {
                attached.arm_overrides = saved;
            }
        }
        Some(StatusUpdate::info(format!(
            "Watch rule \"{}\" {verb}.",
            snapshot.label
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};
    use crate::pty::PtyClient;
    use crate::watch::{WatchAction, WatchBackoff, WatchBudget};

    /// A rule that fires on the very next tick after its pattern appears.
    fn instant_rule(pattern: &str, text: &str) -> WatchRule {
        WatchRule {
            pattern: pattern.to_string(),
            label: "retry".to_string(),
            action: WatchAction::SendText {
                text: text.to_string(),
                append_enter: true,
            },
            backoff: WatchBackoff {
                initial_ms: 0,
                max_ms: 0,
                multiplier: 1.0,
                jitter_ms: 0,
            },
            budget: WatchBudget { max_attempts: 3 },
            cooldown_ms: 60_000,
            ..Default::default()
        }
    }

    /// A PTY child that prints `banner`, switches its terminal to raw mode, and
    /// records every byte typed at it, unechoed, into `log`. Lets a test see
    /// exactly what dux typed and in how many writes.
    ///
    /// `stty raw` has to finish before anything is typed: in cooked mode the
    /// line discipline echoes and line-buffers the input, so a pasted body
    /// without a newline never reaches `cat` and the log stays empty. The
    /// child therefore creates `<log>.ready` once raw mode is in effect, and
    /// [`wait_until_raw`] waits for that file rather than guessing with a
    /// sleep, which under a loaded full-suite run was sometimes too short.
    fn recording_pty(banner: &str, log: &std::path::Path) -> PtyClient {
        let script = format!(
            "printf '%s\\n' '{banner}'; stty raw -echo; : > '{}.ready'; exec cat > '{}'",
            log.display(),
            log.display()
        );
        PtyClient::spawn(
            "sh",
            &["-c".to_string(), script],
            &std::env::temp_dir(),
            24,
            80,
            100,
        )
        .expect("spawn recording pty")
    }

    fn wait_for(mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        cond()
    }

    /// Wait until a [`recording_pty`] child has put its terminal in raw mode.
    /// `exec cat` follows the marker within one shell step, and bytes typed in
    /// that gap wait in the PTY's (now raw) input queue for `cat` to read.
    fn wait_until_raw(log: &std::path::Path) {
        let ready = log.with_extension("log.ready");
        assert!(
            wait_for(|| ready.exists()),
            "the recording child never reached raw mode ({})",
            ready.display()
        );
    }

    fn read_log(path: &std::path::Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_default()
    }

    /// An engine with one claude agent whose slot tab runs `client`, and
    /// `rules` configured for claude.
    fn engine_with_tab(
        client: PtyClient,
        rules: Vec<WatchRule>,
    ) -> (Engine, crate::test_scratch::ScratchDir) {
        let (mut engine, tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feature"));
        let mut claude = crate::config::ProviderCommandConfig {
            command: "claude".to_string(),
            ..Default::default()
        };
        claude.watch = rules;
        engine
            .config
            .providers
            .commands
            .insert("claude".to_string(), claude);
        engine.providers.insert(TabId::new("s1-slot"), client);
        (engine, tmp)
    }

    fn log_path(tmp: &crate::test_scratch::ScratchDir) -> PathBuf {
        tmp.path().join("typed.log")
    }

    #[test]
    fn a_matching_rule_types_the_body_then_submits_it_in_a_later_write() {
        let tmp = crate::test_scratch::ScratchDir::new();
        let log = log_path(&tmp);
        let client = recording_pty("API Error: rate limited", &log);
        let (mut engine, _guard) = engine_with_tab(
            client,
            vec![instant_rule("rate limited", "please continue")],
        );
        assert!(wait_for(|| engine.providers[TabIdRef::new("s1-slot")]
            .scan_recent_lines(WATCH_SCAN_ROWS)
            .contains("rate limited")));
        wait_until_raw(&log);

        // Tick 1 notices the match and schedules; tick 2 fires the body.
        let first = engine.tick_watch_rules();
        assert!(first.is_empty(), "{first:?}");
        let fired = engine.tick_watch_rules();
        assert!(
            fired
                .iter()
                .any(|s| s.message.contains("fired (attempt 1/3)")),
            "{fired:?}"
        );
        let body = b"\x1b[200~please continue\x1b[201~".to_vec();
        assert!(wait_for(|| read_log(&log) == body), "{:?}", read_log(&log));

        // The submit key is held back for the phase delay...
        engine.tick_watch_rules();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(read_log(&log), body, "Enter must not ride with the body");
        // ...then sent on its own.
        std::thread::sleep(Duration::from_millis(MIN_ENTER_PHASE_DELAY_MS));
        engine.tick_watch_rules();
        let mut submitted = body.clone();
        submitted.push(b'\r');
        assert!(
            wait_for(|| read_log(&log) == submitted),
            "{:?}",
            read_log(&log)
        );
        assert!(engine.watch.pending_enters.is_empty());
    }

    #[test]
    fn a_tab_the_user_just_typed_into_is_left_alone() {
        let tmp = crate::test_scratch::ScratchDir::new();
        let log = log_path(&tmp);
        let client = recording_pty("rate limited", &log);
        let (mut engine, _guard) =
            engine_with_tab(client, vec![instant_rule("rate limited", "go")]);
        assert!(wait_for(|| engine.providers[TabIdRef::new("s1-slot")]
            .scan_recent_lines(WATCH_SCAN_ROWS)
            .contains("rate limited")));
        // Raw mode first, or an empty log could mean "nothing got through"
        // rather than "nothing was typed".
        wait_until_raw(&log);
        engine.note_pty_input("s1-slot");
        for _ in 0..3 {
            assert!(engine.tick_watch_rules().is_empty());
        }
        std::thread::sleep(Duration::from_millis(300));
        assert!(read_log(&log).is_empty(), "{:?}", read_log(&log));
        // The rule is still waiting, not spent.
        let rows = engine.watch_rule_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].snapshot.attempts_made, 0);
    }

    #[test]
    fn no_rules_means_no_engine_and_teardown_forgets_the_tab() {
        let client = PtyClient::spawn("cat", &[], &std::env::temp_dir(), 24, 80, 100).expect("cat");
        let (mut engine, _guard) = engine_with_tab(client, Vec::new());
        engine.tick_watch_rules();
        assert!(engine.watch.attached.is_empty(), "no rules, no engine");

        engine.config.providers.commands["claude"].watch = vec![instant_rule("x", "y")];
        engine.tick_watch_rules();
        assert!(engine.watch.attached.contains_key(TabIdRef::new("s1-slot")));
        engine
            .watch
            .pending_enters
            .insert(TabId::new("s1-slot"), Instant::now());

        engine.clear_tab_runtime(TabIdRef::new("s1-slot"));
        assert!(engine.watch.attached.is_empty());
        assert!(engine.watch.pending_enters.is_empty());
    }

    #[test]
    fn an_unchanged_config_keeps_the_engine_and_a_changed_one_rebuilds_it() {
        let client = PtyClient::spawn("cat", &[], &std::env::temp_dir(), 24, 80, 100).expect("cat");
        let (mut engine, _guard) = engine_with_tab(client, vec![instant_rule("x", "y")]);
        engine.tick_watch_rules();
        let tab = TabIdRef::new("s1-slot");
        engine.toggle_watch_rule(tab, 0).expect("toggle");
        assert!(engine.watch.attached[tab].engine.is_disarmed(0));

        // Same rules: the manual disarm survives the next tick.
        engine.tick_watch_rules();
        assert!(engine.watch.attached[tab].engine.is_disarmed(0));

        // A config reload that changes the rule rebuilds the engine armed.
        engine.config.providers.commands["claude"].watch = vec![instant_rule("x", "changed")];
        engine.tick_watch_rules();
        assert!(!engine.watch.attached[tab].engine.is_disarmed(0));
    }

    /// A manual disarm is saved into the session's `watch_rule_arm`, so a
    /// fresh process (restart or hot reload) rebuilds the rule disarmed.
    #[test]
    fn a_toggled_rule_is_persisted_into_the_session_settings() {
        let client = PtyClient::spawn("cat", &[], &std::env::temp_dir(), 24, 80, 100).expect("cat");
        let (mut engine, _guard) = engine_with_tab(client, vec![instant_rule("x", "y")]);
        let session = engine.sessions[0].clone();
        engine.session_store.create_session(&session).unwrap();
        engine.tick_watch_rules();
        let tab = TabIdRef::new("s1-slot");
        engine.toggle_watch_rule(tab, 0).expect("toggle");
        assert_eq!(
            engine
                .session_settings("s1")
                .map(|s| s.watch_rule_arm.get(&0).copied()),
            Some(Some(false))
        );
        let stored = engine.session_store.load_session_settings().unwrap();
        assert_eq!(stored["s1"].watch_rule_arm.get(&0), Some(&false));

        // Forget the live engine; a rebuild from settings comes back disarmed.
        engine.watch.attached.clear();
        engine.tick_watch_rules();
        assert!(engine.watch.attached[tab].engine.is_disarmed(0));

        // Re-arming persists too.
        engine.toggle_watch_rule(tab, 0).expect("toggle back");
        let stored = engine.session_store.load_session_settings().unwrap();
        assert_eq!(stored["s1"].watch_rule_arm.get(&0), Some(&true));
    }

    #[test]
    fn an_invalid_rule_is_reported_and_the_rest_still_load() {
        let client = PtyClient::spawn("cat", &[], &std::env::temp_dir(), 24, 80, 100).expect("cat");
        let (mut engine, _guard) = engine_with_tab(
            client,
            vec![instant_rule("(unclosed", "y"), instant_rule("ok", "y")],
        );
        let statuses = engine.tick_watch_rules();
        assert!(
            statuses
                .iter()
                .any(|s| s.message.contains("pattern rejected")),
            "{statuses:?}"
        );
        assert_eq!(engine.watch_rule_rows().len(), 1);
    }

    #[test]
    fn a_suppression_window_holds_the_tab_then_rebaselines_stale_matches() {
        let tmp = crate::test_scratch::ScratchDir::new();
        let log = log_path(&tmp);
        let client = recording_pty("[task-done]", &log);
        let (mut engine, _guard) =
            engine_with_tab(client, vec![instant_rule(r"\[task-done\]", "/clear")]);
        let tab = TabIdRef::new("s1-slot");
        assert!(wait_for(|| engine.providers[tab]
            .scan_recent_lines(WATCH_SCAN_ROWS)
            .contains("[task-done]")));
        // Raw mode first, or an empty log could mean "nothing got through"
        // rather than "nothing was typed".
        wait_until_raw(&log);
        engine.suppress_watch_rules(tab, Instant::now() + Duration::from_millis(150));
        engine.tick_watch_rules();
        std::thread::sleep(Duration::from_millis(200));
        // Window over: the sentinel already on screen is absorbed, not fired.
        for _ in 0..3 {
            engine.tick_watch_rules();
        }
        std::thread::sleep(Duration::from_millis(200));
        assert!(read_log(&log).is_empty(), "{:?}", read_log(&log));
    }
}
