//! What dux says about a config reload, and on which key.
//!
//! The reload is decided on one surface and owed to both. The web layer tells
//! browsers to refetch BEFORE the drainer has adopted anything, because the
//! companion seam is pre-consume, so that sentence is a claim about a step that
//! may still fail. The apply's own answer therefore has to land on the same key
//! and travel the worker lane both surfaces drain, or a failed reload leaves
//! "reloaded" standing in a browser while the terminal UI alone says it broke.

use crate::engine::StatusUpdate;
use crate::statusline::StatusTone;
use crate::wire::status_keys::CONFIG_RELOAD;

/// What the web says the moment the file has been read and browsers are being
/// told to refetch. Keyed, so the apply's answer replaces it rather than
/// stacking under it.
pub const REFRESHING: &str = "Configuration reloaded; connected browsers are refreshing.";

/// The config was read, validated and adopted.
pub fn applied() -> StatusUpdate {
    StatusUpdate::keyed(
        CONFIG_RELOAD,
        StatusTone::Info,
        "Configuration reloaded. New settings are active now.",
    )
}

/// The config validated but could not be adopted, so the settings already in
/// force are the ones still running.
pub fn apply_failed(error: &str) -> StatusUpdate {
    StatusUpdate::keyed(
        CONFIG_RELOAD,
        StatusTone::Error,
        format!(
            "Config validation passed, but applying it failed: {error}. The settings already in \
             force are unchanged; fix config.toml and reload again."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_outcomes_share_the_reload_key() {
        assert_eq!(applied().key.as_deref(), Some(CONFIG_RELOAD));
        assert_eq!(apply_failed("boom").key.as_deref(), Some(CONFIG_RELOAD));
    }

    #[test]
    fn the_failure_names_the_error_and_a_remedy() {
        let status = apply_failed("right_width_pct out of range");
        assert_eq!(status.tone, StatusTone::Error);
        assert!(status.message.contains("right_width_pct out of range"));
        assert!(status.message.contains("fix config.toml and reload again"));
    }
}
