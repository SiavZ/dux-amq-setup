//! Dux peer router: agent-to-agent message routing between Dux sessions, and
//! the globally locked AMQ registry lifecycle that keeps every Dux store's
//! agent handles unique inside one shared AMQ root.
//!
//! Ported from the fork's monolithic `src/peer.rs`. The logic lives here, in
//! `dux-core`, so both surfaces can use it: the TUI and the web server run the
//! startup reconciliation ([`sync_amq_agents_for_bootstrap`]), every agent
//! launch exports the session identity ([`append_session_env`]), and the
//! `dux peer` CLI ([`run_peer`]) is dispatched from the binary's argument
//! router.
//!
//! The router speaks in [`PeerSession`] rather than [`crate::model::AgentSession`]
//! on purpose. The fork's session carried a persisted immutable
//! `agent_handle`, a `shared_workspace` flag and soft-delete tombstones; upstream's
//! does not yet, and those belong to the shared-workspace port. Everything the
//! router decides is a function of the small projection below, and the one
//! place that builds it from upstream's store is [`session_store`].

mod amq;
mod handle;
mod router;
pub mod session_store;

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub use amq::{
    AmqSyncReport, amq_cleanup_requires_worker, amq_handle_is_exact_owner,
    amq_handle_is_exact_owner_at_root, free_amq_handle, free_amq_handle_at_root, optional_amq_root,
    reserve_and_persist_session, reserve_and_persist_session_at_root, sync_amq_agents,
    sync_amq_agents_for_bootstrap, tombstone_amq_session, tombstone_amq_session_at_root,
};
pub use handle::{
    AGENT_HANDLE_MAX_LEN, amq_handle, derive_agent_handle, is_valid_agent_handle,
    next_unique_agent_handle, normalize_agent_handle,
};
pub use router::run_peer;
pub use session_store::{
    append_session_env, init_for_process, launch_env_for_session, load_or_create_store_id,
};

/// What the peer router needs to know about one Dux agent session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSession {
    /// The Dux session id.
    pub id: String,
    /// Provider name as configured (`claude`, `codex`, ...).
    pub provider: String,
    /// Where the agent runs: its worktree, or its folder for a standalone agent.
    pub directory: String,
    /// The branch the agent tracks, when it has one. An alias for targeting.
    pub branch: Option<String>,
    /// The agent's durable title, when it has one. An alias for targeting.
    pub title: Option<String>,
    /// The agent's AMQ identity: its inbox name under the shared AMQ root.
    pub agent_handle: String,
    /// Several agents share one working directory. Such endpoints are always
    /// routed over AMQ, because Claude Peers keys registrations by directory
    /// and cannot tell them apart.
    pub shared_workspace: bool,
    /// Ordinary-deleted row kept as a tombstone so its handle stays reserved.
    pub deleted: bool,
    /// The agent's process has exited.
    pub exited: bool,
}

impl PeerSession {
    pub fn agent_handle(&self) -> &str {
        &self.agent_handle
    }

    pub fn is_claude(&self) -> bool {
        self.provider == "claude"
    }

    /// A session a message can actually reach: live, and its directory still
    /// exists. Exited agents and agents whose worktree is gone are hidden from
    /// `dux peer list` and cannot be targeted.
    pub fn is_routable(&self) -> bool {
        !self.deleted && !self.exited && Path::new(&self.directory).exists()
    }
}

/// Persistence the AMQ registry lifecycle needs from a Dux session store.
///
/// INTEGRATION: the fork implemented these directly on its `SessionStore`
/// (`load_sessions_including_deleted`, `upsert_session`,
/// `reassign_agent_handle_for_global_backfill`). The shared-workspace port adds
/// the persisted handle and tombstones to upstream's store; the adapter in
/// [`session_store`] is then the only code that changes.
pub trait PeerStore {
    /// Every session this store knows, tombstones included, so a deleted row's
    /// handle is never handed to a new session.
    fn load_sessions_including_deleted(&self) -> Result<Vec<PeerSession>>;

    /// Insert a brand-new session with its reserved handle.
    fn persist_new_session(&self, session: &PeerSession) -> Result<()>;

    /// Compare-and-swap a session's handle during global deconfliction. Called
    /// only while the AMQ registry lock proves `expected` is owned by someone
    /// else and `replacement` is free.
    fn reassign_agent_handle_for_global_backfill(
        &self,
        id: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<()>;
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;

    use super::PeerSession;

    /// A live, non-shared session whose handle is its normalized branch.
    pub(crate) fn session(id: &str, provider: &str, branch: &str, directory: &Path) -> PeerSession {
        PeerSession {
            id: id.to_string(),
            provider: provider.to_string(),
            directory: directory.display().to_string(),
            branch: Some(branch.to_string()),
            title: None,
            agent_handle: super::normalize_agent_handle(branch),
            shared_workspace: false,
            deleted: false,
            exited: false,
        }
    }
}
