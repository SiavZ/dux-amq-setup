//! Per-tab runtime state for the watch engine, owned by
//! [`crate::engine::Engine`] and driven by `Engine::tick_watch_rules`.
//!
//! Keyed by TAB id, not session id: an upstream agent can run several tabs,
//! each with its own provider, and a rule set belongs to the provider whose
//! output it reads.

use std::collections::HashMap;
use std::time::Instant;

use crate::ids::TabId;

use super::{WatchEngine, WatchRule};

/// One tab's attached engine plus the inputs it was built from, so a config
/// reload that changes the rules (or the session settings that add the
/// built-in auto-clear rule) rebuilds it, and one that does not keeps its
/// attempt counters and cooldowns.
pub struct AttachedWatch {
    pub engine: WatchEngine,
    /// The exact rule list the engine was built from (user rules, then the
    /// built-in auto-clear rule when attached).
    pub rules: Vec<WatchRule>,
    /// Arm overrides that were applied at build time, by rule index.
    pub arm_overrides: Vec<(usize, bool)>,
    /// Index of the built-in auto-clear rule, when attached.
    pub auto_clear_idx: Option<usize>,
}

#[derive(Default)]
pub struct WatchRuntime {
    pub attached: HashMap<TabId, AttachedWatch>,
    /// Tabs whose last `send_text` body is typed but whose submit key is
    /// deferred to a later tick (two-phase delivery), with when the body went.
    pub pending_enters: HashMap<TabId, Instant>,
    /// Tabs whose watch rules are paused until the given instant (after an
    /// AMQ inject wrote a postscript that could itself match a rule). When the
    /// window ends the engine rebaselines once, absorbing any stale match.
    pub suppress_until: HashMap<TabId, Instant>,
}

impl WatchRuntime {
    /// Forget everything about a tab. Called from the engine's tab teardown so
    /// a relaunched tab starts with fresh budgets and no stale deferred Enter.
    pub fn forget(&mut self, tab_id: &crate::ids::TabIdRef) {
        self.attached.remove(tab_id);
        self.pending_enters.remove(tab_id);
        self.suppress_until.remove(tab_id);
    }
}
