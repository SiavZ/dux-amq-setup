//! Serializable types describing a single watch rule.

use serde::{Deserialize, Serialize};

/// One watch rule. Loaded from `[[providers.<name>.watch]]` arrays in
/// `config.toml`. Fields default to safe values so partially-specified rules
/// in user configs still load.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchRule {
    /// Regex pattern matched against each row of the agent's visible
    /// terminal viewport. Compiled by the engine at load time; invalid
    /// patterns are reported and the rule is skipped.
    pub pattern: String,
    /// Optional human label used in status messages. If empty, a
    /// truncated copy of `pattern` stands in.
    pub label: String,
    /// Action to take when the pattern matches.
    #[serde(flatten)]
    pub action: WatchAction,
    /// Backoff schedule applied to repeat matches.
    pub backoff: WatchBackoff,
    /// Maximum number of times the rule may fire in one session.
    pub budget: WatchBudget,
    /// If the same rule re-matches within this many milliseconds of its
    /// last action, the engine treats it as the same incident — the
    /// backoff curve is not reset and the budget is not consumed again.
    pub cooldown_ms: u64,
    /// Internal classification for rules constructed by dux itself.
    /// User TOML never sets this; serde skips it so persisted/configured
    /// rules remain forward-compatible.
    #[serde(skip)]
    pub kind: WatchRuleKind,
}

impl Default for WatchRule {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            label: String::new(),
            action: WatchAction::default(),
            backoff: WatchBackoff::default(),
            budget: WatchBudget::default(),
            cooldown_ms: 30_000,
            kind: WatchRuleKind::User,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WatchRuleKind {
    #[default]
    User,
    BuiltInAutoClear,
}

/// Action variants a rule can dispatch when it fires.
///
/// Serialized as a flat TOML form using `action = "<kind>"` as the
/// discriminator and the variant fields as siblings:
///
/// ```toml
/// [[providers.claude.watch]]
/// pattern = "..."
/// action = "send_text"
/// text = "please continue"
/// append_enter = true
/// ```
///
/// The `wait_until_capture` variant pairs a regex capture group with a
/// parser, schedules the fire for the resulting instant, then sends
/// `text` — useful for messages like Claude Code's "5-hour usage limit
/// reached" where the reset time is encoded in the message body.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum WatchAction {
    /// Write `text` (optionally followed by `\r` to simulate Enter) to the
    /// agent's PTY.
    SendText {
        text: String,
        #[serde(default = "default_append_enter")]
        append_enter: bool,
    },
    /// Extract the named capture group from the rule's regex, parse it
    /// using `format`, and schedule the action for the parsed instant.
    /// At fire time, sends `text` like a `send_text` action. If the
    /// capture is missing or unparseable, falls back to the rule's
    /// backoff curve and emits a warning.
    WaitUntilCapture {
        /// Name of the regex capture group holding the time value.
        /// Must match `(?<name>…)` in the `pattern` field.
        capture: String,
        /// Parser kind to apply to the captured string.
        format: WaitFormat,
        /// Text to send once the wait elapses.
        text: String,
        #[serde(default = "default_append_enter")]
        append_enter: bool,
    },
}

impl Default for WatchAction {
    fn default() -> Self {
        Self::SendText {
            text: String::new(),
            append_enter: true,
        }
    }
}

impl WatchAction {
    /// Text payload sent when the action fires. Both variants ship a
    /// text payload — the only difference is *when* the engine fires.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::SendText { text, .. } => text,
            Self::WaitUntilCapture { text, .. } => text,
        }
    }

    /// Whether to append CR to the payload.
    pub(crate) fn append_enter(&self) -> bool {
        match self {
            Self::SendText { append_enter, .. } => *append_enter,
            Self::WaitUntilCapture { append_enter, .. } => *append_enter,
        }
    }
}

fn default_append_enter() -> bool {
    true
}

/// Parser kinds for `WatchAction::WaitUntilCapture`. The captured string
/// is interpreted according to this enum and turned into a future
/// `Instant` for scheduling.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitFormat {
    /// Integer seconds since the Unix epoch (e.g. `1738531200`).
    UnixSeconds,
    /// Integer milliseconds since the Unix epoch.
    UnixMillis,
    /// Wall-clock time of day (`"3pm"`, `"3:00pm"`, `"15:00"`,
    /// `"3:30 pm"`). Optional trailing `(Timezone)` suffix is stripped
    /// and ignored — we treat the time as today's local time and roll
    /// to tomorrow if already past. Timezone-aware parsing is
    /// intentionally out of scope: the user's `TZ` env var typically
    /// matches the displayed timezone, and rolling to tomorrow handles
    /// the most common ambiguity.
    ClockLocal,
    /// Floating-point seconds from now.
    InSeconds,
    /// Floating-point minutes from now.
    InMinutes,
    /// Floating-point hours from now (e.g. `"5"` for "in 5 hours").
    InHours,
}

/// Exponential backoff parameters with jitter.
///
/// Delay for attempt `n` (0-indexed) is
/// `min(initial_ms * multiplier^n, max_ms) + uniform_random(0, jitter_ms)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchBackoff {
    pub initial_ms: u64,
    pub max_ms: u64,
    pub multiplier: f64,
    pub jitter_ms: u64,
}

impl Default for WatchBackoff {
    fn default() -> Self {
        Self {
            initial_ms: 60_000,
            max_ms: 600_000,
            multiplier: 2.0,
            jitter_ms: 5_000,
        }
    }
}

impl WatchBackoff {
    /// Compute the deterministic (jitter-free) component of the delay for
    /// the given attempt number. Capped at `max_ms`.
    pub(crate) fn deterministic_delay_ms(&self, attempt: u32) -> u64 {
        let mult = self.multiplier.max(1.0);
        let raw = (self.initial_ms as f64) * mult.powi(attempt as i32);
        if !raw.is_finite() || raw > self.max_ms as f64 {
            self.max_ms
        } else {
            raw as u64
        }
    }
}

/// How many times a rule may fire before disarming itself for the rest of
/// the session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchBudget {
    pub max_attempts: u32,
}

impl Default for WatchBudget {
    fn default() -> Self {
        Self { max_attempts: 5 }
    }
}
