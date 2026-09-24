//! Exact-owner AMQ inbox release used by hard purge and factory reset.
//!
//! INTEGRATION: free_amq_handle (seedling). This whole module is a stand-in
//! for `crate::peer::amq::{free_amq_handle_at_root, amq_handle_is_exact_owner_at_root,
//! optional_amq_root}` on wt/port-peer, which take a `PeerSession` built with
//! `crate::peer::session_store::peer_session(session, handle, deleted)`. At
//! merge, replace the three bodies below with calls to seedling's functions and
//! delete the local marker/lock code. The stand-in keeps the same fail-closed
//! contract so the purge and reset tests exercise the real semantics:
//!
//! - The shared registry lock (`<root>/meta/config.lock`, `flock` exclusive) is
//!   held for every read and write, the same lock the AMQ wrappers take.
//! - An inbox is released only when its `.dux-amq-source` marker names exactly
//!   this store id and session id. A foreign, legacy (path-only) or missing
//!   marker refuses, so a peer's inbox is never touched.
//! - The handle is dropped from `<root>/meta/config.json` BEFORE the directory
//!   is removed, and a malformed `config.json` refuses before anything changes.
//!
//! One gap versus seedling's version, closed by the swap: it does not stop a
//! recorded AMQ wake process (`wake_pid`) before removing the inbox.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use rustix::fs::{FlockOperation, flock};
use serde::Deserialize;
use serde_json::Value;

use crate::config::DuxPaths;
use crate::model::AgentSession;
use crate::sanitize::for_terminal;

const OWNER_MARKER: &str = ".dux-amq-source";

#[derive(Deserialize)]
struct OwnerMarker {
    store_id: String,
    session_id: String,
}

/// INTEGRATION: free_amq_handle (seedling). Replace with
/// `crate::peer::amq::optional_amq_root(paths)`.
///
/// The AMQ root this store shares, if any: `AMQ_GLOBAL_ROOT`, else `AM_ROOT`,
/// else an `amq` directory that already exists beside the dux home.
pub fn optional_amq_root(paths: &DuxPaths) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AMQ_GLOBAL_ROOT").or_else(|| std::env::var_os("AM_ROOT"))
    {
        return Some(PathBuf::from(path));
    }
    let sibling = paths.root.parent()?.join("amq");
    sibling.exists().then_some(sibling)
}

/// INTEGRATION: free_amq_handle (seedling). Replace with
/// `crate::peer::amq::amq_handle_is_exact_owner_at_root(root, store_id, &peer_session(..))`.
///
/// Read-only: whether `session` exactly owns its inbox under `root`. Used by
/// reset to inventory every handle before freeing any of them.
pub fn amq_handle_is_exact_owner_at_root(
    root: &Path,
    store_id: &str,
    session: &AgentSession,
) -> Result<bool> {
    let _lock = RegistryLock::acquire(root)?;
    Ok(matches!(
        marker_state(root, session.agent_handle())?,
        Marker::Owner(owner) if owner.store_id == store_id && owner.session_id == session.id
    ))
}

/// INTEGRATION: free_amq_handle (seedling). Replace with
/// `crate::peer::amq::free_amq_handle_at_root(root, store_id, &peer_session(..))`.
///
/// Release `session`'s inbox under `root` after exact owner verification. An
/// already-absent inbox is a no-op.
pub fn free_amq_handle_at_root(root: &Path, store_id: &str, session: &AgentSession) -> Result<()> {
    let _lock = RegistryLock::acquire(root)?;
    let handle = session.agent_handle();
    match marker_state(root, handle)? {
        Marker::Free => return Ok(()),
        Marker::Owner(owner) if owner.store_id == store_id && owner.session_id == session.id => {}
        Marker::Owner(_) | Marker::Foreign => bail!(
            "refusing AMQ cleanup for handle {:?}: ownership does not match store/session",
            for_terminal(handle)
        ),
    }
    remove_config_handle(root, handle)?;
    let agent_dir = root.join("agents").join(handle);
    fs::remove_dir_all(&agent_dir).with_context(|| {
        format!(
            "failed to remove owned AMQ inbox {}",
            for_terminal(&agent_dir.display().to_string())
        )
    })
}

enum Marker {
    Free,
    Owner(OwnerMarker),
    Foreign,
}

fn marker_state(root: &Path, handle: &str) -> Result<Marker> {
    let agent_dir = root.join("agents").join(handle);
    let Ok(metadata) = fs::symlink_metadata(&agent_dir) else {
        return Ok(Marker::Free);
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(Marker::Foreign);
    }
    let marker = agent_dir.join(OWNER_MARKER);
    match fs::symlink_metadata(&marker) {
        Ok(meta) if meta.is_file() => {}
        _ => return Ok(Marker::Foreign),
    }
    let raw = fs::read_to_string(&marker)
        .with_context(|| format!("failed to read {}", marker.display()))?;
    Ok(serde_json::from_str::<OwnerMarker>(&raw)
        .map(Marker::Owner)
        .unwrap_or(Marker::Foreign))
}

fn remove_config_handle(root: &Path, handle: &str) -> Result<()> {
    let path = root.join("meta/config.json");
    if !path.exists() {
        return Ok(());
    }
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut config: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let agents = config
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("AMQ config agents is not an array"))?;
    agents.retain(|value| value.as_str() != Some(handle));
    let body = format!("{}\n", serde_json::to_string_pretty(&config)?);
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut file =
        File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    fs::rename(&tmp, &path).with_context(|| format!("failed to replace {}", path.display()))
}

struct RegistryLock {
    file: File,
}

impl RegistryLock {
    fn acquire(root: &Path) -> Result<Self> {
        let meta = root.join("meta");
        fs::create_dir_all(&meta)
            .with_context(|| format!("failed to create {}", meta.display()))?;
        let path = meta.join("config.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("failed to open mandatory AMQ lock {}", path.display()))?;
        crate::io_retry::retry_on_interrupt_errno(|| flock(&file, FlockOperation::LockExclusive))
            .with_context(|| format!("failed to acquire mandatory AMQ lock {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ =
            crate::io_retry::retry_on_interrupt_errno(|| flock(&self.file, FlockOperation::Unlock));
    }
}
