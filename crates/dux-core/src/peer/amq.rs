//! Globally locked AMQ registry lifecycle.
//!
//! One AMQ root can be shared by several Dux stores (several `DUX_HOME`s) and
//! by the `claude-amq` / `codex-amq` wrappers. Every agent directory under
//! `<root>/agents/<handle>` carries an owner marker naming the exact
//! `(store_id, session_id)` that owns it, and every mutation of the registry
//! (`meta/config.json` plus the agent directories) happens under the shared
//! `meta/config.lock` flock the wrappers also take. That is what keeps two
//! stores that both want the handle `agent` from sharing one inbox.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::handle::{for_terminal, normalize_agent_handle, suffixed_handle};
use super::{PeerSession, PeerStore};
use crate::config::DuxPaths;
use crate::logger;

pub(crate) const OWNER_MARKER: &str = ".dux-amq-source";

/// What a reconciliation pass changed in the shared registry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AmqSyncReport {
    pub root: Option<PathBuf>,
    pub configured_agents_added: usize,
    pub stale_config_agents_removed: usize,
    pub ownership_markers_created: usize,
    pub handles_deconflicted: usize,
    /// No AMQ root is configured, so nothing was touched.
    pub skipped: bool,
}

/// The AMQ root this Dux store shares, if any: `AMQ_GLOBAL_ROOT`, else
/// `AM_ROOT`, else an `amq` directory that already exists beside `DUX_HOME`.
pub fn optional_amq_root(paths: &DuxPaths) -> Option<PathBuf> {
    if let Some(path) = env::var_os("AMQ_GLOBAL_ROOT").or_else(|| env::var_os("AM_ROOT")) {
        return Some(PathBuf::from(path));
    }
    if let Some(parent) = paths.root.parent() {
        let sibling = parent.join("amq");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    None
}

pub(crate) fn require_amq_root(paths: &DuxPaths) -> Result<PathBuf> {
    optional_amq_root(paths).ok_or_else(|| {
        anyhow!("AMQ root is not configured; set AMQ_GLOBAL_ROOT or install dux-amq")
    })
}

/// Reconcile the shared AMQ registry from this store's sessions: reserve an
/// owned inbox for every live session, deconflict handles another store
/// already owns, and drop config entries whose owning row is gone.
pub fn sync_amq_agents(paths: &DuxPaths) -> Result<AmqSyncReport> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(AmqSyncReport {
            skipped: true,
            ..AmqSyncReport::default()
        });
    };
    let store_id = super::session_store::load_or_create_store_id(&paths.root)?;
    if !paths.sessions_db_path.exists() {
        return reconcile_amq_root(&root, &store_id, None, &mut []);
    }
    let store = super::session_store::open(paths)?;
    let mut sessions = store.load_sessions_including_deleted()?;
    reconcile_amq_root(&root, &store_id, Some(&store), &mut sessions)
}

/// Startup reconciliation for a surface (TUI or web server). Never fails the
/// boot: a broken or foreign registry is logged and left untouched.
pub fn sync_amq_agents_for_bootstrap(paths: &DuxPaths) {
    match sync_amq_agents(paths) {
        Ok(report) if report.skipped => {
            logger::debug("AMQ registry sync skipped; no AMQ root configured");
        }
        Ok(report) => logger::debug(&format!(
            "AMQ registry synced from Dux sessions: root={} added={} stale_removed={} \
             markers_created={} deconflicted={}",
            for_terminal(
                &report
                    .root
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            ),
            report.configured_agents_added,
            report.stale_config_agents_removed,
            report.ownership_markers_created,
            report.handles_deconflicted,
        )),
        Err(err) => logger::warn(&format!(
            "AMQ startup reconciliation failed; continuing without changing the shared \
             registry: {}",
            for_terminal(&format!("{err:#}"))
        )),
    }
}

/// Whether deleting `session` needs AMQ work that can block (stopping a wake
/// process), so the caller should run it off the UI thread. Unknown or
/// unreadable ownership errs toward the worker.
pub fn amq_cleanup_requires_worker(
    paths: &DuxPaths,
    store_id: &str,
    session: &PeerSession,
) -> bool {
    optional_amq_root(paths)
        .is_some_and(|root| amq_cleanup_requires_worker_at_root(&root, store_id, session))
}

fn amq_cleanup_requires_worker_at_root(root: &Path, store_id: &str, session: &PeerSession) -> bool {
    match marker_state(root, session.agent_handle()) {
        Ok(MarkerState::Owner(owner)) => {
            owner.store_id == store_id && owner.session_id == session.id
        }
        Ok(MarkerState::Free) => false,
        Ok(MarkerState::Legacy(_) | MarkerState::Foreign) | Err(_) => true,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct OwnerMarker {
    pub(crate) store_id: String,
    pub(crate) session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) wake_pid: Option<u32>,
}

pub(crate) enum MarkerState {
    /// No agent directory: the handle is unclaimed.
    Free,
    /// An exact owner marker.
    Owner(OwnerMarker),
    /// A pre-ownership marker that records only a worktree path.
    Legacy(PathBuf),
    /// Anything else: a directory without a marker, a symlink, garbage.
    Foreign,
}

/// The mandatory registry lock, shared with the AMQ wrappers.
pub(crate) struct AmqRegistryLock {
    file: File,
}

impl AmqRegistryLock {
    pub(crate) fn acquire(root: &Path) -> Result<Self> {
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

impl Drop for AmqRegistryLock {
    fn drop(&mut self) {
        let _ =
            crate::io_retry::retry_on_interrupt_errno(|| flock(&self.file, FlockOperation::Unlock));
    }
}

/// Persist a new row and reserve its global AMQ identity before the provider
/// launches. A failed reservation can leave the row for retry, but never an
/// owner marker without its row.
pub fn reserve_and_persist_session(
    paths: &DuxPaths,
    store_id: &str,
    store: &dyn PeerStore,
    session: &mut PeerSession,
) -> Result<()> {
    match optional_amq_root(paths) {
        Some(root) => reserve_and_persist_session_at_root(&root, store_id, store, session),
        None => reserve_and_persist_session_locally(store, session),
    }
}

/// No shared AMQ root: the handle only has to be unique within this store,
/// tombstones included.
fn reserve_and_persist_session_locally(
    store: &dyn PeerStore,
    session: &mut PeerSession,
) -> Result<()> {
    let used = store
        .load_sessions_including_deleted()?
        .into_iter()
        .filter(|row| row.id != session.id)
        .map(|row| row.agent_handle)
        .collect::<HashSet<_>>();
    let base = normalize_agent_handle(session.agent_handle());
    if base.is_empty() {
        bail!("new session has an empty agent handle");
    }
    session.agent_handle = super::handle::next_unique_agent_handle(&base, &used);
    store.persist_new_session(session)
}

pub fn reserve_and_persist_session_at_root(
    root: &Path,
    store_id: &str,
    store: &dyn PeerStore,
    session: &mut PeerSession,
) -> Result<()> {
    let _lock = AmqRegistryLock::acquire(root)?;
    let used = store
        .load_sessions_including_deleted()?
        .into_iter()
        .filter(|row| row.id != session.id)
        .map(|row| row.agent_handle)
        .collect::<HashSet<_>>();
    let handle = claim_handle_locked(root, store_id, session, &used)?;
    session.agent_handle = handle.clone();
    store.persist_new_session(session)?;
    ensure_owner_marker(root, &handle, &owner_for(store_id, session))?;
    register_config_handle(root, &handle)?;
    Ok(())
}

/// Reserve (or confirm) `session`'s inbox without touching any store, and
/// return the handle it now owns. Used at launch so the `DUX_AMQ_HANDLE` a
/// provider sees is always the inbox this session actually owns.
pub(crate) fn claim_handle_at_root(
    root: &Path,
    store_id: &str,
    session: &PeerSession,
    used: &HashSet<String>,
) -> Result<String> {
    let _lock = AmqRegistryLock::acquire(root)?;
    let handle = claim_handle_locked(root, store_id, session, used)?;
    ensure_owner_marker(root, &handle, &owner_for(store_id, session))?;
    register_config_handle(root, &handle)?;
    Ok(handle)
}

fn claim_handle_locked(
    root: &Path,
    store_id: &str,
    session: &PeerSession,
    used: &HashSet<String>,
) -> Result<String> {
    fs::create_dir_all(root.join("agents"))
        .with_context(|| format!("failed to create {}", root.join("agents").display()))?;
    let owner = owner_for(store_id, session);
    let base = normalize_agent_handle(session.agent_handle());
    if base.is_empty() {
        bail!("new session has an empty agent handle");
    }
    if !used.contains(&base) && marker_is_claimable(root, &base, &owner)? {
        Ok(base)
    } else {
        next_global_handle(root, &base, used, &owner)
    }
}

fn owner_for(store_id: &str, session: &PeerSession) -> OwnerMarker {
    OwnerMarker {
        store_id: store_id.to_string(),
        session_id: session.id.clone(),
        wake_pid: None,
    }
}

pub(crate) fn reconcile_amq_root(
    root: &Path,
    store_id: &str,
    store: Option<&dyn PeerStore>,
    sessions: &mut [PeerSession],
) -> Result<AmqSyncReport> {
    fs::create_dir_all(root.join("agents"))
        .with_context(|| format!("failed to create {}", root.join("agents").display()))?;
    let _lock = AmqRegistryLock::acquire(root)?;
    let mut report = AmqSyncReport {
        root: Some(root.to_path_buf()),
        ..AmqSyncReport::default()
    };
    let mut order = (0..sessions.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| sessions[*left].id.cmp(&sessions[*right].id));
    let mut used = sessions
        .iter()
        .map(|session| session.agent_handle().to_string())
        .collect::<HashSet<_>>();

    for index in order {
        // A tombstone keeps its handle reserved but must never get its inbox
        // recreated: that would resurrect delivery to a deleted agent.
        if sessions[index].deleted {
            continue;
        }
        let owner = owner_for(store_id, &sessions[index]);
        let current = sessions[index].agent_handle().to_string();
        let claimable = match marker_state(root, &current)? {
            MarkerState::Free => true,
            MarkerState::Owner(existing) => same_owner(&existing, &owner),
            MarkerState::Legacy(path) => {
                let matches = sessions
                    .iter()
                    .filter(|candidate| paths_equivalent(Path::new(&candidate.directory), &path))
                    .count();
                matches == 1 && paths_equivalent(Path::new(&sessions[index].directory), &path)
            }
            MarkerState::Foreign => false,
        };
        let handle = if claimable {
            current.clone()
        } else {
            let replacement = next_global_handle(root, &current, &used, &owner)?;
            let Some(store) = store else {
                bail!("cannot deconflict an AMQ handle without a session store");
            };
            store.reassign_agent_handle_for_global_backfill(
                &sessions[index].id,
                &current,
                &replacement,
            )?;
            used.insert(replacement.clone());
            sessions[index].agent_handle = replacement.clone();
            report.handles_deconflicted += 1;
            replacement
        };
        match marker_state(root, &handle)? {
            MarkerState::Owner(existing) if same_owner(&existing, &owner) => {}
            MarkerState::Free | MarkerState::Legacy(_) => {
                ensure_owner_marker(root, &handle, &owner)?;
                report.ownership_markers_created += 1;
            }
            MarkerState::Owner(_) | MarkerState::Foreign => {
                bail!("AMQ handle changed owner while the registry lock was held")
            }
        }
    }

    let desired = sessions
        .iter()
        .filter(|session| !session.deleted)
        .map(|session| session.agent_handle().to_string())
        .collect::<BTreeSet<_>>();
    let sessions_by_id = sessions
        .iter()
        .map(|session| (session.id.as_str(), session))
        .collect::<HashMap<_, _>>();
    // Without a store the inventory may be incomplete, so nothing is pruned.
    let inventory_complete = store.is_some();
    let config_path = root.join("meta/config.json");
    let mut config = read_or_create_amq_config(&config_path)?;
    let agents_value = config
        .get_mut("agents")
        .ok_or_else(|| anyhow!("AMQ config missing agents array"))?;
    let mut agents = agents_value
        .as_array()
        .ok_or_else(|| anyhow!("AMQ config agents is not an array"))?
        .iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<_>>();
    let before = agents.len();
    agents.extend(desired);
    report.configured_agents_added = agents.len().saturating_sub(before);
    let before_prune = agents.len();
    // Only entries this store provably owns are candidates for pruning;
    // manual entries and other stores' handles are left alone.
    agents.retain(|handle| match marker_state(root, handle) {
        Ok(MarkerState::Owner(owner)) if owner.store_id == store_id => {
            !inventory_complete
                || sessions_by_id
                    .get(owner.session_id.as_str())
                    .is_some_and(|session| !session.deleted && session.agent_handle() == handle)
        }
        _ => true,
    });
    report.stale_config_agents_removed = before_prune.saturating_sub(agents.len());
    *agents_value = Value::Array(agents.into_iter().map(Value::String).collect());
    write_json_atomic(&config_path, &config)?;
    Ok(report)
}

fn read_or_create_amq_config(path: &Path) -> Result<Value> {
    if path.exists() {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        return Ok(value);
    }
    Ok(json!({
        "version": 1,
        "created_utc": Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        "agents": [],
    }))
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    write_atomic(path, format!("{body}\n").as_bytes())
}

fn same_owner(left: &OwnerMarker, right: &OwnerMarker) -> bool {
    left.store_id == right.store_id && left.session_id == right.session_id
}

pub(crate) fn marker_state(root: &Path, handle: &str) -> Result<MarkerState> {
    let agent_dir = root.join("agents").join(handle);
    let Ok(metadata) = fs::symlink_metadata(&agent_dir) else {
        return Ok(MarkerState::Free);
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(MarkerState::Foreign);
    }
    let marker = agent_dir.join(OWNER_MARKER);
    let Ok(metadata) = fs::symlink_metadata(&marker) else {
        return Ok(MarkerState::Foreign);
    };
    if metadata.file_type().is_symlink() {
        return fs::read_link(&marker)
            .map(MarkerState::Legacy)
            .with_context(|| format!("failed to read legacy marker {}", marker.display()));
    }
    if !metadata.is_file() {
        return Ok(MarkerState::Foreign);
    }
    let raw = fs::read_to_string(&marker)
        .with_context(|| format!("failed to read {}", marker.display()))?;
    match serde_json::from_str::<OwnerMarker>(&raw) {
        Ok(owner) => Ok(MarkerState::Owner(owner)),
        Err(_) if !raw.trim().is_empty() => Ok(MarkerState::Legacy(PathBuf::from(raw.trim()))),
        Err(_) => Ok(MarkerState::Foreign),
    }
}

fn marker_is_claimable(root: &Path, handle: &str, owner: &OwnerMarker) -> Result<bool> {
    Ok(match marker_state(root, handle)? {
        MarkerState::Free => true,
        MarkerState::Owner(existing) => same_owner(&existing, owner),
        MarkerState::Legacy(_) | MarkerState::Foreign => false,
    })
}

pub(crate) fn ensure_owner_marker(root: &Path, handle: &str, owner: &OwnerMarker) -> Result<()> {
    ensure_owner_marker_with(root, handle, owner, write_atomic)
}

fn ensure_owner_marker_with(
    root: &Path,
    handle: &str,
    owner: &OwnerMarker,
    write_marker: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    let agent_dir = root.join("agents").join(handle);
    let body = serde_json::to_vec(owner)?;
    let created = match marker_state(root, handle)? {
        MarkerState::Owner(existing) if same_owner(&existing, owner) => {
            #[cfg(unix)]
            fs::set_permissions(&agent_dir, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to secure {}", agent_dir.display()))?;
            return Ok(());
        }
        MarkerState::Free => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            builder.mode(0o700);
            builder
                .create(&agent_dir)
                .with_context(|| format!("failed to reserve {}", agent_dir.display()))?;
            true
        }
        MarkerState::Legacy(_) => false,
        MarkerState::Owner(_) | MarkerState::Foreign => {
            bail!("AMQ handle is owned by another session")
        }
    };
    #[cfg(unix)]
    fs::set_permissions(&agent_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure {}", agent_dir.display()))?;
    let result = write_marker(&agent_dir.join(OWNER_MARKER), &body);
    if let Err(err) = result {
        if created {
            fs::remove_dir(&agent_dir).with_context(|| {
                format!(
                    "failed to remove partial AMQ reservation {}",
                    agent_dir.display()
                )
            })?;
        }
        return Err(err);
    }
    Ok(())
}

fn next_global_handle(
    root: &Path,
    base: &str,
    used: &HashSet<String>,
    owner: &OwnerMarker,
) -> Result<String> {
    for ordinal in 2u64.. {
        let candidate = suffixed_handle(base, ordinal);
        if !used.contains(&candidate) && marker_is_claimable(root, &candidate, owner)? {
            return Ok(candidate);
        }
    }
    unreachable!("u64 handle suffix space exhausted")
}

fn paths_equivalent(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub(crate) fn register_config_handle(root: &Path, handle: &str) -> Result<()> {
    fs::create_dir_all(root.join("meta"))
        .with_context(|| format!("failed to create {}", root.join("meta").display()))?;
    let path = root.join("meta/config.json");
    let mut config = read_or_create_amq_config(&path)?;
    let agents = config
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("AMQ config agents is not an array"))?;
    if !agents.iter().any(|value| value.as_str() == Some(handle)) {
        agents.push(Value::String(handle.to_string()));
        agents.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    }
    write_json_atomic(&path, &config)
}

fn remove_config_handle(root: &Path, handle: &str) -> Result<()> {
    fs::create_dir_all(root.join("meta"))
        .with_context(|| format!("failed to create {}", root.join("meta").display()))?;
    let path = root.join("meta/config.json");
    let mut config = read_or_create_amq_config(&path)?;
    let agents = config
        .get_mut("agents")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("AMQ config agents is not an array"))?;
    agents.retain(|value| value.as_str() != Some(handle));
    write_json_atomic(&path, &config)
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let result = (|| {
        let mut file =
            File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(body)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| {
            format!(
                "failed to replace {} with {}",
                path.display(),
                tmp.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Stop delivery and remove a session from the live registry while keeping
/// its inbox and exact owner marker as an ordinary-delete tombstone. Failures
/// are logged, never propagated: a registry problem must not block deleting
/// the agent locally.
// INTEGRATION: no caller yet. Engine session delete must call this (fork: src/app/sessions.rs delete path, gated by amq_handle_is_exact_owner_at_root) (evergreen)
pub fn tombstone_amq_session(
    paths: &DuxPaths,
    store_id: &str,
    session: &PeerSession,
) -> Result<()> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(());
    };
    if let Err(err) = tombstone_amq_session_at_root(&root, store_id, session) {
        logger::warn(&format!(
            "AMQ cleanup failed during session deletion; continuing with the local tombstone: \
             session={} handle={} error={}",
            for_terminal(&session.id),
            for_terminal(session.agent_handle()),
            for_terminal(&format!("{err:#}"))
        ));
    }
    Ok(())
}

pub fn tombstone_amq_session_at_root(
    root: &Path,
    store_id: &str,
    session: &PeerSession,
) -> Result<()> {
    // The lock is released before waiting for the wake process to exit, so a
    // slow wake never stalls every other registry user.
    let wake_pid = {
        let _lock = AmqRegistryLock::acquire(root)?;
        let owner = match marker_state(root, session.agent_handle()) {
            Ok(MarkerState::Owner(owner))
                if owner.store_id == store_id && owner.session_id == session.id =>
            {
                owner
            }
            Ok(_) => return Ok(()),
            Err(err) => {
                logger::warn(&format!(
                    "could not verify the AMQ owner marker during deletion; leaving it \
                     untouched: session={} handle={} error={}",
                    for_terminal(&session.id),
                    for_terminal(session.agent_handle()),
                    for_terminal(&format!("{err:#}"))
                ));
                return Ok(());
            }
        };
        let wake_pid = owner.wake_pid;
        let mut tombstone = owner;
        tombstone.wake_pid = None;
        write_atomic(
            &root
                .join("agents")
                .join(session.agent_handle())
                .join(OWNER_MARKER),
            &serde_json::to_vec(&tombstone)?,
        )?;
        remove_config_handle(root, session.agent_handle())?;
        wake_pid
    };
    if let Some(pid) = wake_pid {
        terminate_wake_pid(pid, root, session.agent_handle())?;
    }
    Ok(())
}

/// Verify exact ownership, stop wake delivery, remove the inbox, and release
/// a global handle. The hard-purge path is the production caller.
// INTEGRATION: no caller yet. `dux purge` and `dux reset --all` must call this before deleting rows (evergreen / purge worker)
pub fn free_amq_handle(paths: &DuxPaths, store_id: &str, session: &PeerSession) -> Result<()> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(());
    };
    if !root.exists() {
        return Ok(());
    }
    free_amq_handle_at_root(&root, store_id, session)
}

/// Read-only ownership classification, used by reset to inventory every
/// handle before freeing any. Foreign and legacy registrations are preserved.
pub fn amq_handle_is_exact_owner(
    paths: &DuxPaths,
    store_id: &str,
    session: &PeerSession,
) -> Result<bool> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(false);
    };
    if !root.exists() {
        return Ok(false);
    }
    amq_handle_is_exact_owner_at_root(&root, store_id, session)
}

pub fn amq_handle_is_exact_owner_at_root(
    root: &Path,
    store_id: &str,
    session: &PeerSession,
) -> Result<bool> {
    let _lock = AmqRegistryLock::acquire(root)?;
    Ok(matches!(
        marker_state(root, session.agent_handle())?,
        MarkerState::Owner(owner)
            if owner.store_id == store_id && owner.session_id == session.id
    ))
}

pub fn free_amq_handle_at_root(root: &Path, store_id: &str, session: &PeerSession) -> Result<()> {
    let wake_pid = {
        let _lock = AmqRegistryLock::acquire(root)?;
        if matches!(
            marker_state(root, session.agent_handle())?,
            MarkerState::Free
        ) {
            return Ok(());
        }
        let owner = exact_owner(root, store_id, session)?;
        let wake_pid = owner.wake_pid;
        let mut stopped = owner;
        stopped.wake_pid = None;
        write_atomic(
            &root
                .join("agents")
                .join(session.agent_handle())
                .join(OWNER_MARKER),
            &serde_json::to_vec(&stopped)?,
        )?;
        remove_config_handle(root, session.agent_handle())?;
        wake_pid
    };
    if let Some(pid) = wake_pid {
        terminate_wake_pid(pid, root, session.agent_handle())?;
    }
    let _lock = AmqRegistryLock::acquire(root)?;
    exact_owner(root, store_id, session)?;
    let agent_dir = root.join("agents").join(session.agent_handle());
    fs::remove_dir_all(&agent_dir).with_context(|| {
        format!(
            "failed to remove owned AMQ inbox {}",
            for_terminal(&agent_dir.display().to_string())
        )
    })
}

fn exact_owner(root: &Path, store_id: &str, session: &PeerSession) -> Result<OwnerMarker> {
    match marker_state(root, session.agent_handle())? {
        MarkerState::Owner(owner)
            if owner.store_id == store_id && owner.session_id == session.id =>
        {
            Ok(owner)
        }
        MarkerState::Owner(_) | MarkerState::Legacy(_) | MarkerState::Foreign => bail!(
            "refusing AMQ cleanup for handle {:?}: ownership does not match store/session",
            for_terminal(session.agent_handle())
        ),
        MarkerState::Free => bail!(
            "refusing AMQ cleanup for handle {:?}: ownership marker is missing",
            for_terminal(session.agent_handle())
        ),
    }
}

fn terminate_wake_pid(pid: u32, root: &Path, handle: &str) -> Result<()> {
    let Some(rustix_pid) = rustix::process::Pid::from_raw(pid as i32) else {
        bail!("invalid recorded AMQ wake PID {pid}");
    };
    if rustix::process::test_kill_process(rustix_pid).is_err() {
        return Ok(());
    }
    // A recorded PID may have been reused by an unrelated process. Only a
    // process whose argv positively identifies it as this handle's wake is
    // signalled; anything else is a safe leak, never a wrong kill.
    if !is_amq_wake_process(pid, root, handle) {
        logger::warn(&format!(
            "recorded wake PID {pid} no longer identifies an AMQ wake process; leaving it \
             untouched"
        ));
        return Ok(());
    }
    rustix::process::kill_process(rustix_pid, rustix::process::Signal::TERM)
        .with_context(|| format!("failed to terminate AMQ wake PID {pid}"))?;
    if wait_for_process_exit(rustix_pid, Duration::from_millis(750)) {
        return Ok(());
    }
    rustix::process::kill_process(rustix_pid, rustix::process::Signal::KILL)
        .with_context(|| format!("failed to kill AMQ wake PID {pid}"))?;
    if wait_for_process_exit(rustix_pid, Duration::from_millis(750)) {
        Ok(())
    } else {
        bail!("AMQ wake PID {pid} remained alive after SIGKILL")
    }
}

pub(crate) fn is_amq_wake_process(pid: u32, root: &Path, handle: &str) -> bool {
    let sys_pid = sysinfo::Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[sys_pid]),
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    let Some(process) = system.process(sys_pid) else {
        return false;
    };
    amq_wake_command_matches(process.cmd(), root, handle)
}

fn amq_wake_command_matches(command: &[OsString], root: &Path, handle: &str) -> bool {
    let has_pair = |flag: &str, value: &OsStr| {
        command
            .windows(2)
            .any(|pair| pair[0] == OsStr::new(flag) && pair[1] == value)
    };
    command
        .iter()
        .any(|part| part.to_string_lossy().contains("amq"))
        && command.iter().any(|part| part == OsStr::new("wake"))
        && has_pair("--me", OsStr::new(handle))
        && has_pair("--root", root.as_os_str())
}

fn wait_for_process_exit(pid: rustix::process::Pid, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if rustix::process::test_kill_process(pid).is_err() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    rustix::process::test_kill_process(pid).is_err()
}

#[cfg(test)]
#[path = "amq_tests.rs"]
mod tests;
