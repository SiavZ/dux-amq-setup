//! What dux says when it cannot read an agent's changed files, and when it can
//! again.
//!
//! Two pollers ask the same question: the terminal UI's own changed-files
//! refresh and the web's changes service. They used to answer differently, the
//! web raising a warning through its own side door and the terminal UI saying
//! nothing at all, so the same failing repository produced two different
//! screens. The streak, the threshold, the key and both sentences live here,
//! and the engine owns the one instance both pollers feed.
//!
//! Feeding one tracker from two pollers is deliberately harmless: the streak
//! simply climbs faster, the warning is emitted on the one crossing, and the
//! recovery fires once.

use std::collections::{HashMap, HashSet};

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;

/// Consecutive failures before a warning is raised.
///
/// git fails transiently all the time (an index lock held by the agent's own
/// commit, a directory mid-rename), and a toast per blip is noise. A streak is
/// the signal that something is actually wrong.
pub const ERROR_WARN_THRESHOLD: u32 = 3;

/// The keyed-status key for one agent's changed-files escalation. The warning
/// and its recovery share it, so the surfaces replace rather than stack.
pub fn warn_key(session_id: &str) -> String {
    format!("changes-error:{session_id}")
}

/// The per-agent failure streaks, and which agents have actually been warned
/// about.
#[derive(Debug, Default)]
pub struct ChangedFilesFailures {
    streaks: HashMap<String, u32>,
    warned: HashSet<String>,
}

impl ChangedFilesFailures {
    /// Record one failed read. Answers with the keyed warning on the crossing
    /// and with nothing on every other failure, so a blip stays silent and a
    /// standing failure is reported exactly once.
    pub fn record_failure(&mut self, session_id: &str, message: &str) -> Option<StatusUpdate> {
        let streak = self.streaks.entry(session_id.to_string()).or_insert(0);
        *streak += 1;
        if *streak != ERROR_WARN_THRESHOLD {
            return None;
        }
        self.warned.insert(session_id.to_string());
        Some(StatusUpdate::keyed(
            warn_key(session_id),
            StatusTone::Warning,
            format!(
                "Changed files for this agent are temporarily unavailable (git busy): {message}"
            ),
        ))
    }

    /// Record one successful read. Answers with the keyed recovery ONLY when a
    /// warning was actually shown, so a blip that never raised one does not
    /// leave an orphaned "it is back" behind it.
    pub fn record_success(&mut self, session_id: &str) -> Option<StatusUpdate> {
        self.streaks.remove(session_id);
        if !self.warned.remove(session_id) {
            return None;
        }
        Some(StatusUpdate::keyed(
            warn_key(session_id),
            StatusTone::Info,
            "Changed files are back: git had been failing for this worktree and the latest check \
             succeeded."
                .to_string(),
        ))
    }

    /// Forget an agent entirely, for a session that is gone.
    pub fn forget(&mut self, session_id: &str) {
        self.streaks.remove(session_id);
        self.warned.remove(session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blip_stays_silent_and_a_streak_warns_once() {
        let mut failures = ChangedFilesFailures::default();
        for _ in 1..ERROR_WARN_THRESHOLD {
            assert!(failures.record_failure("s1", "index.lock exists").is_none());
        }
        let warning = failures
            .record_failure("s1", "index.lock exists")
            .expect("the streak crosses the threshold");
        assert_eq!(warning.tone, StatusTone::Warning);
        assert_eq!(warning.key.as_deref(), Some(warn_key("s1").as_str()));
        assert!(warning.message.contains("index.lock exists"));
        assert!(
            failures.record_failure("s1", "index.lock exists").is_none(),
            "a standing failure is reported once, not once per cycle"
        );
    }

    #[test]
    fn the_recovery_only_fires_where_a_warning_was_shown() {
        let mut failures = ChangedFilesFailures::default();
        assert!(failures.record_failure("s1", "boom").is_none());
        assert!(
            failures.record_success("s1").is_none(),
            "a blip that never warned owes no recovery"
        );

        for _ in 0..ERROR_WARN_THRESHOLD {
            failures.record_failure("s2", "boom");
        }
        let recovery = failures
            .record_success("s2")
            .expect("a warned agent recovers out loud");
        assert_eq!(recovery.tone, StatusTone::Info);
        assert_eq!(recovery.key.as_deref(), Some(warn_key("s2").as_str()));
        assert!(failures.record_success("s2").is_none(), "and says so once");
    }

    /// Both pollers feed one tracker, so the streak climbs faster and the
    /// warning still lands exactly once.
    #[test]
    fn two_pollers_feeding_one_tracker_still_warn_once() {
        let mut failures = ChangedFilesFailures::default();
        let mut warnings = 0;
        for _ in 0..(ERROR_WARN_THRESHOLD * 2) {
            if failures.record_failure("s1", "boom").is_some() {
                warnings += 1;
            }
        }
        assert_eq!(warnings, 1);
    }
}
