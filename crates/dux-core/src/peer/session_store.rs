//! The adapter between upstream's session store and the peer router, plus
//! the per-launch `DUX_*` identity environment.
//!
//! A session's AMQ identity is its persisted, immutable `agent_handle` column
//! (`AgentSession::agent_handle`), assigned once at create, never rewritten
//! except by the global-backfill compare-and-swap, and kept on the row's
//! soft-delete tombstone so a deleted agent's handle is never reused. This
//! module only projects rows into the router's [`PeerSession`] and exports
//! the launch environment.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use rustix::fs::{FlockOperation, flock};

use super::handle::{for_terminal, is_valid_agent_handle};
use super::{PeerSession, PeerStore};
use crate::config::DuxPaths;
use crate::logger;
use crate::model::{AgentSession, SessionStatus};
use crate::storage::SessionStore;

const STORE_ID_FILE: &str = "store-id";
const STORE_ID_LOCK: &str = ".store-id.lock";

/// Load the stable identifier of one `DUX_HOME`, creating it atomically on
/// first use. AMQ owner markers name `(store_id, session_id)`, so two Dux
/// homes sharing one AMQ root can never claim each other's inboxes. The lock
/// covers the first write and every reader, so nobody sees a partial id.
pub fn load_or_create_store_id(dux_home: &Path) -> Result<String> {
    fs::create_dir_all(dux_home)
        .with_context(|| format!("failed to create {}", dux_home.display()))?;
    let _lock = FileLock::acquire(&dux_home.join(STORE_ID_LOCK))?;
    let path = dux_home.join(STORE_ID_FILE);
    if path.exists() {
        return read_store_id(&path);
    }
    let id = uuid::Uuid::new_v4().to_string();
    write_private_atomic(&path, format!("{id}\n").as_bytes())?;
    Ok(id)
}

fn read_store_id(path: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed = uuid::Uuid::parse_str(raw.trim()).with_context(|| {
        format!(
            "DUX_HOME metadata corruption: {} does not contain a valid store id",
            path.display()
        )
    })?;
    Ok(parsed.to_string())
}

/// Append the Dux identity every agent PTY exports. The AMQ wrappers read
/// these to join the inbox Dux reserved (all three set together, or none),
/// and `dux peer send` reads them to know who is sending.
pub fn append_session_env(env: &mut Vec<(String, String)>, session: &PeerSession, store_id: &str) {
    env.push(("DUX_SESSION_ID".to_string(), session.id.clone()));
    env.push(("DUX_STORE_ID".to_string(), store_id.to_string()));
    env.push(("DUX_PROVIDER".to_string(), session.provider.clone()));
    env.push((
        "DUX_AMQ_HANDLE".to_string(),
        session.agent_handle().to_string(),
    ));
}

/// The `DUX_*` identity variables for launching `session`, reserving its
/// handle on first launch. Best effort: a failure is logged and yields no
/// variables, so a broken session store or AMQ root never blocks starting an agent
/// (the wrappers then fall back to their own identity, as before dux set one).
pub fn launch_env_for_session(paths: &DuxPaths, session: &AgentSession) -> Vec<(String, String)> {
    match try_launch_env_for_session(paths, session) {
        Ok(env) => env,
        Err(err) => {
            logger::warn(&format!(
                "could not resolve the Dux peer identity for session {}; launching without \
                 DUX_* variables: {}",
                for_terminal(&session.id),
                for_terminal(&format!("{err:#}"))
            ));
            Vec::new()
        }
    }
}

fn try_launch_env_for_session(
    paths: &DuxPaths,
    session: &AgentSession,
) -> Result<Vec<(String, String)>> {
    launch_env_with_root(paths, session, launch_amq_root().as_deref())
}

/// The AMQ root agent launches reserve their inbox in, fixed once per process
/// by [`init_for_process`]. Library code, and therefore every test that builds
/// a launch request, sees `None` unless a binary entry point opted in, so a
/// test run on a machine that exports `AMQ_GLOBAL_ROOT` never writes into the
/// user's real registry.
static LAUNCH_AMQ_ROOT: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Opt this process into the shared AMQ registry: remember the root for agent
/// launches and reconcile the registry once from this store's sessions. The
/// `dux` TUI and `dux server` entry points call it before their engine boots.
/// Never fails the boot.
pub fn init_for_process(paths: &DuxPaths) {
    let _ = LAUNCH_AMQ_ROOT.set(super::amq::optional_amq_root(paths));
    super::amq::sync_amq_agents_for_bootstrap(paths);
}

fn launch_amq_root() -> Option<PathBuf> {
    LAUNCH_AMQ_ROOT.get().cloned().flatten()
}

/// Resolve (and on first launch assign) the session's handle. With an AMQ
/// root, also reserve the inbox up front so the handle the provider is told is
/// one this session provably owns: a foreign owner moves it to a suffixed
/// handle, persisted before the launch. This replaces the fork's reservation
/// at row creation, and covers agents created before the port too.
fn launch_env_with_root(
    paths: &DuxPaths,
    session: &AgentSession,
    amq_root: Option<&Path>,
) -> Result<Vec<(String, String)>> {
    let store_id = load_or_create_store_id(&paths.root)?;
    let store = SessionStore::open(&paths.sessions_db_path)
        .with_context(|| format!("failed to open {}", paths.sessions_db_path.display()))?;
    let rows = store.load_sessions_including_deleted()?;
    // The stored row is authoritative: a global backfill may have moved this
    // session's handle since the in-memory copy was loaded. A session with no
    // row yet (a test, or a launch racing its own create) uses its own value.
    let handle = rows.iter().find(|row| row.id == session.id).map_or_else(
        || session.agent_handle().to_string(),
        |row| row.agent_handle().to_string(),
    );
    let mut peer = peer_session(session, handle, false);
    if let Some(root) = amq_root {
        let used = rows
            .iter()
            .filter(|row| row.id != session.id)
            .map(|row| row.agent_handle().to_string())
            .collect::<HashSet<_>>();
        let owned = super::amq::claim_handle_at_root(root, &store_id, &peer, &used)?;
        if owned != peer.agent_handle {
            store.reassign_agent_handle_for_global_backfill(
                &peer.id,
                &peer.agent_handle,
                &owned,
            )?;
            peer.agent_handle = owned;
        }
    }
    let mut env = Vec::new();
    append_session_env(&mut env, &peer, &store_id);
    Ok(env)
}

/// Project an upstream session into what the router needs.
pub fn peer_session(session: &AgentSession, agent_handle: String, deleted: bool) -> PeerSession {
    PeerSession {
        id: session.id.clone(),
        provider: session.provider.as_str().to_string(),
        directory: session.directory().to_string(),
        branch: session.branch_name().map(str::to_string),
        title: session.title.clone(),
        agent_handle,
        shared_workspace: session.shared_workspace(),
        deleted,
        exited: matches!(session.status, SessionStatus::Exited),
    }
}

/// Open this `DUX_HOME`'s session store as a [`PeerStore`].
pub fn open(paths: &DuxPaths) -> Result<SqlitePeerStore> {
    let store = SessionStore::open(&paths.sessions_db_path)
        .with_context(|| format!("failed to open {}", paths.sessions_db_path.display()))?;
    Ok(SqlitePeerStore { store })
}

/// Upstream's [`SessionStore`] seen through the [`PeerStore`] seam.
pub struct SqlitePeerStore {
    store: SessionStore,
}

impl SqlitePeerStore {
    /// Live (non-deleted) sessions only.
    pub fn load_sessions(&self) -> Result<Vec<PeerSession>> {
        Ok(self
            .load_sessions_including_deleted()?
            .into_iter()
            .filter(|session| !session.deleted)
            .collect())
    }
}

impl PeerStore for SqlitePeerStore {
    fn load_sessions_including_deleted(&self) -> Result<Vec<PeerSession>> {
        Ok(self
            .store
            .load_sessions_including_deleted()?
            .iter()
            .map(|row| peer_session(row, row.agent_handle().to_string(), row.is_deleted()))
            .collect())
    }

    /// Record a brand-new session with its reserved handle (fork: the
    /// reservation's `upsert_session` inserted the row). An existing row's
    /// handle is immutable outside the global-backfill swap, so a differing
    /// value is refused; an identical one is a no-op. A row that does not
    /// exist yet is created as a folder agent at `directory`, which is all a
    /// router-driven reservation knows about it.
    fn persist_new_session(&self, session: &PeerSession) -> Result<()> {
        ensure!(
            is_valid_agent_handle(&session.agent_handle),
            "refusing to persist invalid agent handle {:?}",
            for_terminal(&session.agent_handle)
        );
        let stored = self
            .store
            .load_sessions_including_deleted()?
            .into_iter()
            .find(|row| row.id == session.id);
        if let Some(row) = stored {
            ensure!(
                row.agent_handle() == session.agent_handle,
                "refusing to change immutable agent handle for session {:?}",
                for_terminal(&session.id)
            );
            return Ok(());
        }
        let now = chrono::Utc::now();
        let row = AgentSession {
            id: session.id.clone(),
            agent_handle: session.agent_handle.clone(),
            shared_workspace: session.shared_workspace,
            deleted_at: None,
            slot_tab_id: format!("{}-slot", session.id),
            provider: crate::model::ProviderKind::new(&session.provider),
            workspace: crate::model::AgentWorkspace::Folder(crate::model::FolderWorkspace {
                folder_path: session.directory.clone(),
            }),
            title: session.title.clone(),
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: false,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        };
        self.store.create_session(&row)
    }

    fn reassign_agent_handle_for_global_backfill(
        &self,
        id: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<()> {
        self.store
            .reassign_agent_handle_for_global_backfill(id, expected, replacement)
    }
}

struct FileLock {
    file: File,
}

impl FileLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        crate::io_retry::retry_on_interrupt_errno(|| flock(&file, FlockOperation::LockExclusive))
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ =
            crate::io_retry::retry_on_interrupt_errno(|| flock(&self.file, FlockOperation::Unlock));
    }
}

fn write_private_atomic(path: &Path, body: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.tmp.{}.{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        file.write_all(body)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to sync {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("failed to replace {}", path.display()))?;
        if let Ok(dir) = File::open(dir) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentWorkspace, FolderWorkspace, ProviderKind};
    use chrono::Utc;
    use tempfile::tempdir;

    pub(crate) fn folder_session(id: &str, provider: &str, dir: &Path) -> AgentSession {
        AgentSession {
            id: id.to_string(),
            agent_handle: crate::model::normalize_agent_handle(id),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: format!("{id}-slot"),
            provider: ProviderKind::new(provider),
            workspace: AgentWorkspace::Folder(FolderWorkspace {
                folder_path: dir.display().to_string(),
            }),
            title: Some(id.to_string()),
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_focused_tab: None,
        }
    }

    fn paths_in(dir: &Path) -> DuxPaths {
        let root = dir.join("home");
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root,
        }
    }

    #[test]
    fn store_id_is_durable_and_reused() {
        let dir = tempdir().unwrap();
        let first = load_or_create_store_id(dir.path()).unwrap();
        let second = load_or_create_store_id(dir.path()).unwrap();
        assert_eq!(first, second);
        uuid::Uuid::parse_str(&first).unwrap();
    }

    #[test]
    fn corrupt_store_id_fails_closed() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(STORE_ID_FILE), "not-a-uuid\n").unwrap();
        let error = load_or_create_store_id(dir.path()).unwrap_err().to_string();
        assert!(error.contains("DUX_HOME metadata corruption"));
    }

    #[test]
    fn pty_env_exports_store_session_and_immutable_handle() {
        let session = PeerSession {
            id: "session-a".to_string(),
            provider: "claude".to_string(),
            directory: "/w/different-path-name".to_string(),
            branch: None,
            title: None,
            agent_handle: "stable-handle".to_string(),
            shared_workspace: false,
            deleted: false,
            exited: false,
        };
        let mut env = Vec::new();

        append_session_env(&mut env, &session, "store-a");

        assert!(env.contains(&("DUX_STORE_ID".to_string(), "store-a".to_string())));
        assert!(env.contains(&("DUX_SESSION_ID".to_string(), "session-a".to_string())));
        assert!(env.contains(&("DUX_PROVIDER".to_string(), "claude".to_string())));
        assert!(env.contains(&("DUX_AMQ_HANDLE".to_string(), "stable-handle".to_string())));
    }

    /// The persisted `agent_handle` column is the peer identity: assigned once
    /// and locally unique at create (tombstones included), kept on a soft
    /// delete, immutable except through the global-backfill compare-and-swap.
    #[test]
    fn stored_handles_are_unique_kept_on_tombstones_and_only_swapped_by_backfill() {
        let dir = tempdir().unwrap();
        let paths = paths_in(dir.path());
        fs::create_dir_all(&paths.root).unwrap();
        let a = dir.path().join("x/agent");
        let b = dir.path().join("y/agent");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        for (id, provider, dir) in [("s1", "claude", &a), ("s2", "codex", &b)] {
            let mut session = folder_session(id, provider, dir);
            session.agent_handle = "agent".to_string();
            store.assign_unique_agent_handle(&mut session).unwrap();
            store.create_session(&session).unwrap();
        }

        let peer_store = open(&paths).unwrap();
        let mut handles = peer_store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.agent_handle)
            .collect::<Vec<_>>();
        handles.sort();
        assert_eq!(handles, ["agent", "agent-2"]);

        // Deleting the agent keeps its handle reserved as a tombstone.
        store.soft_delete_session("s1").unwrap();
        let all = peer_store.load_sessions_including_deleted().unwrap();
        let tomb = all.iter().find(|s| s.id == "s1").unwrap();
        assert!(tomb.deleted);
        assert_eq!(tomb.agent_handle, "agent");
        assert!(
            peer_store
                .load_sessions()
                .unwrap()
                .iter()
                .all(|s| s.id != "s1")
        );

        // Stored handles are immutable except through the compare-and-swap.
        let s2 = all
            .iter()
            .find(|s| s.id == "s2")
            .unwrap()
            .agent_handle
            .clone();
        assert!(
            peer_store
                .persist_new_session(&PeerSession {
                    agent_handle: "other".to_string(),
                    ..all.iter().find(|s| s.id == "s2").unwrap().clone()
                })
                .is_err()
        );
        assert!(
            peer_store
                .reassign_agent_handle_for_global_backfill("s2", "wrong", "agent-9")
                .is_err()
        );
        peer_store
            .reassign_agent_handle_for_global_backfill("s2", &s2, "agent-9")
            .unwrap();
        let reloaded = open(&paths).unwrap().load_sessions().unwrap();
        assert_eq!(reloaded[0].agent_handle, "agent-9");
    }

    #[test]
    fn launch_env_exports_a_stable_identity_across_launches() {
        let dir = tempdir().unwrap();
        let paths = paths_in(dir.path());
        let worktree = dir.path().join("Feature Login");
        fs::create_dir_all(&worktree).unwrap();
        let mut session = folder_session("s1", "claude", &worktree);
        session.agent_handle = crate::model::derive_agent_handle(session.directory(), "", "s1");
        fs::create_dir_all(&paths.root).unwrap();
        SessionStore::open(&paths.sessions_db_path)
            .unwrap()
            .create_session(&session)
            .unwrap();

        let first = try_launch_env_for_session(&paths, &session).unwrap();
        let second = try_launch_env_for_session(&paths, &session).unwrap();

        assert_eq!(first, second);
        let get = |key: &str| {
            first
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(get("DUX_AMQ_HANDLE"), "feature-login");
        assert_eq!(get("DUX_SESSION_ID"), "s1");
        assert_eq!(get("DUX_PROVIDER"), "claude");
        assert_eq!(
            get("DUX_STORE_ID"),
            load_or_create_store_id(&paths.root).unwrap()
        );
    }

    #[test]
    fn launch_with_amq_root_moves_off_a_foreign_owned_inbox_and_persists_it() {
        use super::super::amq::{MarkerState, OwnerMarker, ensure_owner_marker, marker_state};
        let dir = tempdir().unwrap();
        let paths = paths_in(dir.path());
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("agents")).unwrap();
        ensure_owner_marker(
            &root,
            "agent",
            &OwnerMarker {
                store_id: "another-store".to_string(),
                session_id: "theirs".to_string(),
                wake_pid: None,
            },
        )
        .unwrap();
        let worktree = dir.path().join("agent");
        fs::create_dir_all(&worktree).unwrap();
        let mut session = folder_session("s1", "claude", &worktree);
        session.agent_handle = "agent".to_string();
        fs::create_dir_all(&paths.root).unwrap();
        SessionStore::open(&paths.sessions_db_path)
            .unwrap()
            .create_session(&session)
            .unwrap();

        let env = launch_env_with_root(&paths, &session, Some(&root)).unwrap();
        let handle = env
            .iter()
            .find(|(k, _)| k == "DUX_AMQ_HANDLE")
            .map(|(_, v)| v.clone())
            .unwrap();

        assert_eq!(handle, "agent-2");
        match marker_state(&root, "agent-2").unwrap() {
            MarkerState::Owner(owner) => assert_eq!(owner.session_id, "s1"),
            _ => panic!("inbox was not reserved for the launching session"),
        }
        assert!(
            fs::read_to_string(root.join("meta/config.json"))
                .unwrap()
                .contains("\"agent-2\"")
        );
        // The reassigned handle is durable: a relaunch without the root
        // still exports it.
        let again = launch_env_with_root(&paths, &session, None).unwrap();
        assert!(again.contains(&("DUX_AMQ_HANDLE".to_string(), "agent-2".to_string())));
    }
}
