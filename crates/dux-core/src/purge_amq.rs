//! Exact-owner AMQ inbox release used by hard purge and factory reset.
//!
//! A thin adapter over the peer router's AMQ lifecycle
//! (`crate::peer::amq`), which owns the registry lock, the ownership marker
//! and wake-process termination. Purge and reset hold an upstream
//! [`AgentSession`]; this projects it into the router's `PeerSession` with
//! the session's own immutable `agent_handle` and tombstone state, so both
//! callers share one implementation of the fail-closed contract:
//!
//! - The shared registry lock (`<root>/meta/config.lock`, `flock` exclusive)
//!   is held for every read and write, the same lock the AMQ wrappers take.
//! - An inbox is released only when its `.dux-amq-source` marker names
//!   exactly this store id and session id. A foreign, legacy (path-only) or
//!   missing marker refuses, so a peer's inbox is never touched.
//! - A recorded `amq wake` process is stopped before the inbox goes, so it
//!   cannot recreate the directory behind the purge.
//! - An already-absent inbox is a no-op.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::DuxPaths;
use crate::model::AgentSession;

fn peer_view(session: &AgentSession) -> crate::peer::PeerSession {
    crate::peer::session_store::peer_session(
        session,
        session.agent_handle().to_string(),
        session.is_deleted(),
    )
}

/// The AMQ root this store shares, if any: `AMQ_GLOBAL_ROOT`, else `AM_ROOT`,
/// else an `amq` directory that already exists beside the dux home.
pub fn optional_amq_root(paths: &DuxPaths) -> Option<PathBuf> {
    crate::peer::optional_amq_root(paths)
}

/// Read-only: whether `session` exactly owns its inbox under `root`. Used by
/// reset to inventory every handle before freeing any of them.
pub fn amq_handle_is_exact_owner_at_root(
    root: &Path,
    store_id: &str,
    session: &AgentSession,
) -> Result<bool> {
    crate::peer::amq_handle_is_exact_owner_at_root(root, store_id, &peer_view(session))
}

/// Release `session`'s inbox under `root` after exact owner verification,
/// stopping its recorded wake process first. An already-absent inbox is a
/// no-op.
pub fn free_amq_handle_at_root(root: &Path, store_id: &str, session: &AgentSession) -> Result<()> {
    crate::peer::free_amq_handle_at_root(root, store_id, &peer_view(session))
}
