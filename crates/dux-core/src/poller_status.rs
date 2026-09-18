//! What dux says when one of its background workers falls over or never
//! starts.
//!
//! A poller that dies and comes back, and one that could not be spawned at all,
//! used to be log lines: the user saw a list that stopped updating and had
//! nothing on screen to tell them why. Both are facts about the process rather
//! than about a surface, so they travel the engine's worker lane and both
//! surfaces say them in the same words.
//!
//! The two pollers that answer the same question (the terminal UI's own
//! changed-files sweep and the web's changes service) share a key, so a process
//! running both reports one restart rather than two.

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;

/// The keyed-status key for one background worker's health. A restart and a
/// failure to start are outcomes for the same worker, so they replace each
/// other rather than stacking.
pub fn key(label: &str) -> String {
    format!("poller:{label}")
}

/// The label the changed-files pollers share. There are two of them, the
/// terminal UI's sweep and the web's changes service, and they keep one list up
/// to date between them, so one restart is one sentence.
pub const CHANGED_FILES_LABEL: &str = "changed-files-poller";

/// What a user loses while the changed-files pollers are down. Named here
/// because both pollers report under it.
pub const CHANGED_FILES_FEATURE: &str = "changed-file updates";

/// A worker died and dux started it again. `feature` is the consumer-facing
/// noun phrase for what it keeps up to date.
pub fn restarted(label: &str, feature: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        key(label),
        StatusTone::Warning,
        format!(
            "dux restarted the background worker behind {feature} after it failed. What it shows \
             may briefly lag; the reason is in dux.log."
        ),
    )
}

/// A worker could not be started at all, so the feature behind it is gone for
/// this run of dux.
pub fn spawn_failed(label: &str, feature: &str, error: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        key(label),
        StatusTone::Warning,
        format!(
            "dux could not start the background worker behind {feature}: {error}. It stays off \
             until you restart dux."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_outcomes_for_one_worker_share_its_key() {
        assert_eq!(
            restarted("pr-sync", "pull request status updates").key,
            spawn_failed("pr-sync", "pull request status updates", "boom").key
        );
        assert_ne!(
            restarted("pr-sync", "a").key,
            restarted("branch-sync", "a").key
        );
    }

    #[test]
    fn a_restart_names_the_feature_and_where_the_reason_is() {
        let status = restarted(CHANGED_FILES_LABEL, CHANGED_FILES_FEATURE);
        assert_eq!(status.tone, StatusTone::Warning);
        assert!(status.message.contains(CHANGED_FILES_FEATURE));
        assert!(status.message.contains("dux.log"));
    }

    #[test]
    fn a_failure_to_start_says_the_feature_is_off_until_a_restart() {
        let status = spawn_failed("branch-sync", "branch status updates", "too many threads");
        assert!(status.message.contains("branch status updates"));
        assert!(status.message.contains("too many threads"));
        assert!(status.message.contains("until you restart dux"));
    }
}
