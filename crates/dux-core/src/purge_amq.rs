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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AgentSession, AgentWorkspace, BranchProvenance, ManagedWorkspace, ProviderKind,
        SessionStatus,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::{Command, Stdio};

    fn session(id: &str, handle: &str) -> AgentSession {
        let now = chrono::Utc::now();
        AgentSession {
            id: id.to_string(),
            agent_handle: handle.to_string(),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: format!("slot-{id}"),
            provider: ProviderKind::new("claude"),
            workspace: AgentWorkspace::Managed(ManagedWorkspace {
                project_id: "p1".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: handle.to_string(),
                initial_branch: handle.to_string(),
                branch_provenance: BranchProvenance::CreatedByDux,
                worktree_path: format!("/tmp/{handle}"),
            }),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    }

    fn claim(root: &Path, handle: &str, store: &str, session_id: &str, wake_pid: Option<u32>) {
        let dir = root.join("agents").join(handle);
        fs::create_dir_all(&dir).unwrap();
        let mut marker = serde_json::json!({ "store_id": store, "session_id": session_id });
        if let Some(pid) = wake_pid {
            marker["wake_pid"] = pid.into();
        }
        fs::write(dir.join(".dux-amq-source"), marker.to_string()).unwrap();
        fs::create_dir_all(root.join("meta")).unwrap();
        fs::write(
            root.join("meta/config.json"),
            serde_json::json!({ "agents": [handle] }).to_string(),
        )
        .unwrap();
    }

    /// Purge frees an inbox through the peer router, which stops the agent's
    /// recorded `amq wake` process first. The purge worker's original
    /// stand-in removed the inbox but left that process running, and a live
    /// wake recreates the directory it watches, so the purge would not stick.
    /// Pins that the adapter reaches the real implementation.
    #[test]
    fn purge_free_stops_the_recorded_wake_and_removes_the_owned_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("amq");
        let fake_amq = dir.path().join("amq-wake-test");
        fs::write(
            &fake_amq,
            "#!/bin/sh\ntrap 'exit 0' TERM\nwhile :; do :; done\n",
        )
        .unwrap();
        fs::set_permissions(&fake_amq, fs::Permissions::from_mode(0o755)).unwrap();
        let child = Command::new(&fake_amq)
            .args(["wake", "--me", "agent", "--root"])
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        // Reap concurrently, as a real parent would: an unreaped child stays a
        // zombie that still answers kill(0), so the free could never observe
        // the exit it is waiting for.
        let (exited_tx, exited_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = child.wait_with_output();
            let _ = exited_tx.send(());
        });
        let s = session("session-a", "agent");
        claim(&root, "agent", "store-a", &s.id, Some(pid));

        let freed = free_amq_handle_at_root(&root, "store-a", &s);
        // The fake wake spins until signalled, so it exits only if the free
        // stopped it. Bounded, so a regression fails instead of hanging.
        let stopped = exited_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_ok();
        if !stopped {
            if let Some(p) = rustix::process::Pid::from_raw(pid as i32) {
                let _ = rustix::process::kill_process(p, rustix::process::Signal::KILL);
            }
        }

        freed.expect("an exactly owned inbox is freed");
        assert!(
            stopped,
            "the recorded wake process must be stopped by the free"
        );
        assert!(!root.join("agents/agent").exists(), "owned inbox removed");
        let config = fs::read_to_string(root.join("meta/config.json")).unwrap();
        assert!(!config.contains("\"agent\""), "handle dropped from config");
    }

    /// A handle this store does not own is refused and left intact.
    #[test]
    fn purge_free_refuses_a_foreign_owner() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("amq");
        claim(&root, "agent", "store-b", "theirs", None);
        let s = session("mine", "agent");

        assert!(!amq_handle_is_exact_owner_at_root(&root, "store-a", &s).unwrap());
        assert!(free_amq_handle_at_root(&root, "store-a", &s).is_err());
        assert!(root.join("agents/agent/.dux-amq-source").exists());
    }
}
