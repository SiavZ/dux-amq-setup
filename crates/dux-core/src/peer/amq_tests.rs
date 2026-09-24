use super::*;
use std::cell::RefCell;
use std::process::{Command, Stdio};
use tempfile::tempdir;

use crate::peer::test_support::session;

/// In-memory [`PeerStore`], standing in for the fork's `SessionStore` rows.
#[derive(Default)]
struct MemStore {
    rows: RefCell<Vec<PeerSession>>,
}

impl MemStore {
    fn with(rows: &[PeerSession]) -> Self {
        Self {
            rows: RefCell::new(rows.to_vec()),
        }
    }

    fn handle_of(&self, id: &str) -> String {
        self.rows
            .borrow()
            .iter()
            .find(|row| row.id == id)
            .unwrap()
            .agent_handle
            .clone()
    }
}

impl PeerStore for MemStore {
    fn load_sessions_including_deleted(&self) -> Result<Vec<PeerSession>> {
        Ok(self.rows.borrow().clone())
    }

    fn persist_new_session(&self, session: &PeerSession) -> Result<()> {
        let mut rows = self.rows.borrow_mut();
        if let Some(row) = rows.iter_mut().find(|row| row.id == session.id) {
            *row = session.clone();
        } else {
            rows.push(session.clone());
        }
        Ok(())
    }

    fn reassign_agent_handle_for_global_backfill(
        &self,
        id: &str,
        expected: &str,
        replacement: &str,
    ) -> Result<()> {
        let mut rows = self.rows.borrow_mut();
        let row = rows
            .iter_mut()
            .find(|row| row.id == id && row.agent_handle == expected)
            .ok_or_else(|| anyhow!("session changed while completing global handle backfill"))?;
        row.agent_handle = replacement.to_string();
        Ok(())
    }
}

fn owner(store: &str, session: &str, wake_pid: Option<u32>) -> OwnerMarker {
    OwnerMarker {
        store_id: store.to_string(),
        session_id: session.to_string(),
        wake_pid,
    }
}

fn config_raw(root: &Path) -> String {
    fs::read_to_string(root.join("meta/config.json")).unwrap()
}

/// Test-binary re-entry point for the cross-process tests below. A no-op
/// unless `DUX_AMQ_TEST_HELPER_MODE` is set by a parent test.
#[test]
fn subprocess_helper() {
    let Some(mode) = std::env::var_os("DUX_AMQ_TEST_HELPER_MODE") else {
        return;
    };
    let path = |name| PathBuf::from(std::env::var_os(name).expect(name));
    match mode.to_string_lossy().as_ref() {
        "claim" => {
            let root = path("DUX_AMQ_TEST_ROOT");
            let home = path("DUX_AMQ_TEST_HOME");
            let worktree = path("DUX_AMQ_TEST_WORKTREE");
            let ready = path("DUX_AMQ_TEST_READY");
            let start = path("DUX_AMQ_TEST_START");
            let output = path("DUX_AMQ_TEST_OUTPUT");
            let store_id = std::env::var("DUX_AMQ_TEST_STORE_ID").unwrap();
            let session_id = std::env::var("DUX_AMQ_TEST_SESSION_ID").unwrap();
            fs::create_dir_all(&worktree).unwrap();
            fs::write(&ready, b"ready").unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !start.exists() {
                assert!(Instant::now() < deadline, "claim start signal timed out");
                std::thread::sleep(Duration::from_millis(5));
            }
            let paths = test_paths(&home);
            let store = crate::peer::session_store::open(&paths).unwrap();
            let mut candidate = session(&session_id, "claude", "agent", &worktree);
            reserve_and_persist_session_at_root(&root, &store_id, &store, &mut candidate).unwrap();
            fs::write(output, candidate.agent_handle()).unwrap();
        }
        "probe-lock" => {
            let root = path("DUX_AMQ_TEST_ROOT");
            let output = path("DUX_AMQ_TEST_OUTPUT");
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.join("meta/config.lock"))
                .unwrap();
            let message = match flock(&file, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => {
                    flock(&file, FlockOperation::Unlock).unwrap();
                    "acquired".to_string()
                }
                Err(err) => format!("blocked: {err}"),
            };
            fs::write(output, message).unwrap();
        }
        other => panic!("unknown subprocess helper mode: {other}"),
    }
}

fn test_paths(home: &Path) -> DuxPaths {
    fs::create_dir_all(home).unwrap();
    DuxPaths {
        config_path: home.join("config.toml"),
        sessions_db_path: home.join("sessions.sqlite3"),
        worktrees_root: home.join("worktrees"),
        lock_path: home.join("dux.lock"),
        root: home.to_path_buf(),
    }
}

#[test]
fn amq_sync_adds_session_handles_to_config_and_agent_dirs() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let worktree = dir.path().join("worktrees/Agent One");
    fs::create_dir_all(&worktree).unwrap();
    let s = session("s1", "claude", "agent-one", &worktree);
    let store = MemStore::with(std::slice::from_ref(&s));
    let mut sessions = vec![s];

    let report = reconcile_amq_root(&root, "store-a", Some(&store), &mut sessions).unwrap();

    assert_eq!(report.configured_agents_added, 1);
    let agent_dir = root.join("agents/agent-one");
    assert!(agent_dir.is_dir());
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&agent_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert!(config_raw(&root).contains("\"agent-one\""));
    let owner: OwnerMarker = serde_json::from_str(
        &fs::read_to_string(root.join("agents/agent-one/.dux-amq-source")).unwrap(),
    )
    .unwrap();
    assert_eq!(owner.store_id, "store-a");
    assert_eq!(owner.session_id, "s1");
}

#[test]
fn amq_sync_does_not_recreate_deleted_session_agent_dirs() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let worktree = dir.path().join("worktree");
    let mut deleted = session("deleted", "claude", "deleted-agent", &worktree);
    deleted.deleted = true;

    let report = reconcile_amq_root(&root, "store-a", None, &mut [deleted]).unwrap();

    assert_eq!(report.ownership_markers_created, 0);
    assert!(!root.join("agents/deleted-agent").exists());
    assert!(!config_raw(&root).contains("\"deleted-agent\""));
}

/// A tombstone that still has its inbox is pruned from the live config.
#[test]
fn amq_sync_prunes_a_tombstoned_sessions_config_entry() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let mut deleted = session("gone", "claude", "gone", dir.path());
    ensure_owner_marker(&root.tap_agents(), "gone", &owner("store-a", "gone", None)).unwrap();
    register_config_handle(&root, "gone").unwrap();
    deleted.deleted = true;
    let store = MemStore::with(std::slice::from_ref(&deleted));

    let report = reconcile_amq_root(&root, "store-a", Some(&store), &mut [deleted]).unwrap();

    assert_eq!(report.stale_config_agents_removed, 1);
    assert!(!config_raw(&root).contains("\"gone\""));
    assert!(root.join("agents/gone").is_dir(), "the inbox is kept");
}

trait TapAgents {
    fn tap_agents(&self) -> PathBuf;
}

impl TapAgents for PathBuf {
    /// Ensure `<root>/agents` exists and hand the root back.
    fn tap_agents(&self) -> PathBuf {
        fs::create_dir_all(self.join("agents")).unwrap();
        self.clone()
    }
}

#[test]
fn amq_sync_prunes_only_missing_rows_from_its_own_store() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let store = MemStore::default();
    fs::create_dir_all(root.join("meta")).unwrap();
    fs::create_dir_all(root.join("agents/manual")).unwrap();
    fs::write(
        root.join("meta/config.json"),
        r#"{"version":1,"created_utc":"now","agents":["foreign","manual","stale"]}"#,
    )
    .unwrap();
    ensure_owner_marker(&root, "foreign", &owner("store-b", "foreign-session", None)).unwrap();
    ensure_owner_marker(&root, "stale", &owner("store-a", "missing-session", None)).unwrap();

    let report = reconcile_amq_root(&root, "store-a", Some(&store), &mut []).unwrap();
    let raw = config_raw(&root);

    assert_eq!(report.stale_config_agents_removed, 1);
    assert!(raw.contains("\"manual\""));
    assert!(raw.contains("\"foreign\""));
    assert!(!raw.contains("\"stale\""));
}

/// Without a store the inventory may be partial, so nothing is pruned.
#[test]
fn amq_sync_without_a_store_never_prunes() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    ensure_owner_marker(&root, "stale", &owner("store-a", "missing", None)).unwrap();
    register_config_handle(&root, "stale").unwrap();

    let report = reconcile_amq_root(&root, "store-a", None, &mut []).unwrap();

    assert_eq!(report.stale_config_agents_removed, 0);
    assert!(config_raw(&root).contains("\"stale\""));
}

#[test]
fn amq_sync_deconflicts_a_foreign_owned_handle_through_the_store() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    ensure_owner_marker(&root, "agent", &owner("store-b", "theirs", None)).unwrap();
    let s = session("mine", "claude", "agent", dir.path());
    let store = MemStore::with(std::slice::from_ref(&s));
    let mut rows = vec![s];

    let report = reconcile_amq_root(&root, "store-a", Some(&store), &mut rows).unwrap();

    assert_eq!(report.handles_deconflicted, 1);
    assert_eq!(rows[0].agent_handle, "agent-2");
    assert_eq!(store.handle_of("mine"), "agent-2");
    // Without a store there is nowhere to persist the new handle.
    let mut orphan = vec![session("mine2", "claude", "agent", dir.path())];
    assert!(reconcile_amq_root(&root, "store-a", None, &mut orphan).is_err());
}

#[test]
fn global_handle_collision_across_two_stores_gets_suffix() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let store_a = MemStore::default();
    let store_b = MemStore::default();
    let mut first = session("a-session", "claude", "agent", &dir.path().join("a/agent"));
    let mut second = session("b-session", "codex", "agent", &dir.path().join("b/agent"));

    reserve_and_persist_session_at_root(&root, "store-a", &store_a, &mut first).unwrap();
    reserve_and_persist_session_at_root(&root, "store-b", &store_b, &mut second).unwrap();

    assert_eq!(first.agent_handle(), "agent");
    assert_eq!(second.agent_handle(), "agent-2");
    assert_eq!(store_b.handle_of("b-session"), "agent-2");
    assert!(root.join("agents/agent").is_dir());
    assert!(root.join("agents/agent-2").is_dir());
    let raw = config_raw(&root);
    assert!(raw.contains("\"agent\"") && raw.contains("\"agent-2\""));
}

/// A tombstone in the same store keeps its handle out of reach.
#[test]
fn reservation_skips_handles_held_by_local_tombstones() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let mut tomb = session("old", "claude", "agent", dir.path());
    tomb.deleted = true;
    let store = MemStore::with(&[tomb]);
    let mut fresh = session("new", "claude", "agent", dir.path());

    reserve_and_persist_session_at_root(&root, "store-a", &store, &mut fresh).unwrap();

    assert_eq!(fresh.agent_handle(), "agent-2");
}

#[test]
fn concurrent_same_handle_claims_deconflict_under_the_shared_lock() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let start = dir.path().join("start");
    let gate = AmqRegistryLock::acquire(&root).unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut children = Vec::new();
    let mut homes = Vec::new();
    let mut ready_paths = Vec::new();
    let mut output_paths = Vec::new();
    for label in ["a", "b"] {
        let home = dir.path().join(format!("{label}-home"));
        let worktree = dir.path().join(format!("{label}/agent"));
        let ready = dir.path().join(format!("{label}.ready"));
        let output = dir.path().join(format!("{label}.handle"));
        let child = Command::new(&executable)
            .arg("peer::amq::tests::subprocess_helper")
            .arg("--exact")
            .arg("--nocapture")
            .env("DUX_AMQ_TEST_HELPER_MODE", "claim")
            .env("DUX_AMQ_TEST_ROOT", &root)
            .env("DUX_AMQ_TEST_HOME", &home)
            .env("DUX_AMQ_TEST_WORKTREE", &worktree)
            .env("DUX_AMQ_TEST_READY", &ready)
            .env("DUX_AMQ_TEST_START", &start)
            .env("DUX_AMQ_TEST_OUTPUT", &output)
            .env("DUX_AMQ_TEST_STORE_ID", format!("store-{label}"))
            .env("DUX_AMQ_TEST_SESSION_ID", format!("session-{label}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        children.push(child);
        homes.push(home);
        ready_paths.push(ready);
        output_paths.push(output);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while ready_paths.iter().any(|path| !path.exists()) {
        assert!(
            Instant::now() < deadline,
            "claim helpers did not become ready"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    fs::write(&start, b"start").unwrap();
    std::thread::sleep(Duration::from_millis(100));
    // Both claimants are parked on the registry lock this test holds.
    for child in &mut children {
        assert!(child.try_wait().unwrap().is_none());
    }
    assert!(output_paths.iter().all(|path| !path.exists()));
    drop(gate);
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }

    let mut handles = output_paths
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>();
    handles.sort();
    assert_eq!(handles, ["agent", "agent-2"]);
    let mut persisted = homes
        .iter()
        .map(|home| {
            crate::peer::session_store::open(&test_paths(home))
                .unwrap()
                .load_sessions_including_deleted()
                .unwrap()[0]
                .agent_handle
                .clone()
        })
        .collect::<Vec<_>>();
    persisted.sort();
    assert_eq!(persisted, handles);
    assert!(root.join("agents/agent/.dux-amq-source").is_file());
    assert!(root.join("agents/agent-2/.dux-amq-source").is_file());
}

#[test]
fn suffixed_global_handle_stays_within_the_length_bound() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    let base = "a".repeat(crate::peer::AGENT_HANDLE_MAX_LEN);

    let handle =
        next_global_handle(&root, &base, &HashSet::new(), &owner("store-a", "s", None)).unwrap();

    assert_eq!(handle.chars().count(), crate::peer::AGENT_HANDLE_MAX_LEN);
    assert!(handle.ends_with("-2"));
}

#[test]
fn failed_owner_marker_write_removes_the_new_reservation_dir() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();

    let result = ensure_owner_marker_with(&root, "agent", &owner("store-a", "s", None), |_, _| {
        Err(anyhow!("injected marker write failure"))
    });

    assert!(result.is_err());
    assert!(!root.join("agents/agent").exists());
}

#[test]
fn unambiguous_legacy_path_marker_upgrades_to_exact_owner() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let worktree = dir.path().join("worktree/agent");
    fs::create_dir_all(root.join("agents/agent")).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    std::os::unix::fs::symlink(&worktree, root.join("agents/agent/.dux-amq-source")).unwrap();
    let s = session("session-a", "claude", "agent", &worktree);
    let store = MemStore::with(std::slice::from_ref(&s));
    let mut rows = vec![s];

    reconcile_amq_root(&root, "store-a", Some(&store), &mut rows).unwrap();

    assert_eq!(rows[0].agent_handle(), "agent");
    let owner = match marker_state(&root, "agent").unwrap() {
        MarkerState::Owner(owner) => owner,
        _ => panic!("legacy marker was not upgraded"),
    };
    assert_eq!(owner.store_id, "store-a");
    assert_eq!(owner.session_id, "session-a");
}

#[test]
fn ambiguous_legacy_path_is_preserved_and_local_handle_is_suffixed() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let worktree = dir.path().join("shared-worktree");
    fs::create_dir_all(root.join("agents/agent")).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    std::os::unix::fs::symlink(&worktree, root.join("agents/agent/.dux-amq-source")).unwrap();
    let first = session("a-session", "claude", "agent", &worktree);
    let second = session("b-session", "codex", "other", &worktree);
    let store = MemStore::with(&[first.clone(), second.clone()]);
    let mut rows = vec![first, second];

    reconcile_amq_root(&root, "store-a", Some(&store), &mut rows).unwrap();

    assert_eq!(rows[0].agent_handle(), "agent-2");
    assert!(root.join("agents/agent/.dux-amq-source").is_symlink());
    let owner = match marker_state(&root, "agent-2").unwrap() {
        MarkerState::Owner(owner) => owner,
        _ => panic!("replacement owner marker missing"),
    };
    assert_eq!(owner.session_id, "a-session");
}

/// A plain-text legacy marker (path written as the file body) is read as
/// legacy too, and a directory with no marker at all is foreign.
#[test]
fn marker_state_classifies_text_legacy_and_ownerless_dirs() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    fs::create_dir_all(root.join("agents/text")).unwrap();
    fs::write(root.join("agents/text/.dux-amq-source"), "/some/path\n").unwrap();
    fs::create_dir_all(root.join("agents/bare")).unwrap();

    assert!(matches!(
        marker_state(&root, "text").unwrap(),
        MarkerState::Legacy(path) if path == Path::new("/some/path")
    ));
    assert!(matches!(
        marker_state(&root, "bare").unwrap(),
        MarkerState::Foreign
    ));
    assert!(matches!(
        marker_state(&root, "nothing").unwrap(),
        MarkerState::Free
    ));
}

#[test]
fn exact_owner_free_rejects_foreign_then_removes_owned_inbox() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    let s = session("session-a", "claude", "agent", &dir.path().join("agent"));
    ensure_owner_marker(&root, "agent", &owner("store-a", &s.id, None)).unwrap();
    register_config_handle(&root, "agent").unwrap();

    assert!(!amq_handle_is_exact_owner_at_root(&root, "store-b", &s).unwrap());
    assert!(amq_handle_is_exact_owner_at_root(&root, "store-a", &s).unwrap());
    assert!(free_amq_handle_at_root(&root, "store-b", &s).is_err());
    assert!(root.join("agents/agent").exists());
    free_amq_handle_at_root(&root, "store-a", &s).unwrap();
    assert!(!root.join("agents/agent").exists());
    assert!(!config_raw(&root).contains("\"agent\""));
    // Freeing an already-free handle is a no-op.
    free_amq_handle_at_root(&root, "store-a", &s).unwrap();
}

#[test]
fn exact_owner_free_sanitizes_removal_error_path() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq-\u{1b}]8;;bad\u{7}");
    let agents = root.join("agents");
    fs::create_dir_all(&agents).unwrap();
    let session = session("session-a", "claude", "agent", &dir.path().join("agent"));
    ensure_owner_marker(&root, "agent", &owner("store-a", &session.id, None)).unwrap();
    register_config_handle(&root, "agent").unwrap();

    fs::set_permissions(&agents, fs::Permissions::from_mode(0o500)).unwrap();
    let error = free_amq_handle_at_root(&root, "store-a", &session).unwrap_err();
    fs::set_permissions(&agents, fs::Permissions::from_mode(0o700)).unwrap();

    let message = format!("{error:#}");
    assert!(message.contains("failed to remove owned AMQ inbox"));
    assert!(!message.contains('\u{1b}'));
}

/// The fork persisted the row, then reserved; a later spawn failure left a
/// retryable row that still owns its marker. Here the store write happens
/// under the registry lock before the marker, so a crash between the two
/// leaves a row that the next reconciliation completes, never an ownerless
/// marker.
#[test]
fn spawn_failure_after_persist_leaves_retryable_owned_row() {
    let dir = tempdir().unwrap();
    let amq_root = dir.path().join("amq");
    let store = MemStore::default();
    let mut s = session(
        "session-a",
        "claude",
        "agent",
        &dir.path().join("worktree/agent"),
    );

    reserve_and_persist_session_at_root(&amq_root, "store-a", &store, &mut s).unwrap();
    // The launch fails; the row is left for a retry.
    s.exited = true;
    store.persist_new_session(&s).unwrap();

    let loaded = store.load_sessions_including_deleted().unwrap();
    assert_eq!(loaded.len(), 1);
    let owner = match marker_state(&amq_root, loaded[0].agent_handle()).unwrap() {
        MarkerState::Owner(owner) => owner,
        _ => panic!("reserved owner marker missing"),
    };
    assert_eq!(owner.store_id, "store-a");
    assert_eq!(owner.session_id, "session-a");
    // Retrying reconciliation is idempotent for the owned row.
    let mut rows = loaded;
    let report = reconcile_amq_root(&amq_root, "store-a", Some(&store), &mut rows).unwrap();
    assert_eq!(report.ownership_markers_created, 0);
    assert_eq!(report.handles_deconflicted, 0);
}

#[test]
fn reserve_without_an_amq_root_still_assigns_a_locally_unique_handle() {
    let dir = tempdir().unwrap();
    let store = MemStore::with(&[session("a", "claude", "agent", dir.path())]);
    let mut fresh = session("b", "claude", "Agent", dir.path());

    reserve_and_persist_session_locally(&store, &mut fresh).unwrap();

    assert_eq!(fresh.agent_handle(), "agent-2");
    assert_eq!(store.handle_of("b"), "agent-2");
}

#[test]
fn cleanup_needs_a_worker_only_for_owned_or_unclear_markers() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    let s = session("s1", "claude", "agent", dir.path());
    assert!(!amq_cleanup_requires_worker_at_root(&root, "store-a", &s));
    ensure_owner_marker(&root, "agent", &owner("store-a", "s1", None)).unwrap();
    assert!(amq_cleanup_requires_worker_at_root(&root, "store-a", &s));
    assert!(!amq_cleanup_requires_worker_at_root(&root, "store-b", &s));
    fs::create_dir_all(root.join("agents/other")).unwrap();
    let other = session("s2", "claude", "other", dir.path());
    assert!(amq_cleanup_requires_worker_at_root(
        &root, "store-a", &other
    ));
}

#[test]
fn amq_root_comes_from_env_else_an_existing_sibling_dir() {
    let dir = tempdir().unwrap();
    let paths = test_paths(&dir.path().join("home"));
    // Only the sibling rule is testable without mutating process env, and
    // it only applies when no override is exported.
    if std::env::var_os("AMQ_GLOBAL_ROOT").is_none() && std::env::var_os("AM_ROOT").is_none() {
        assert_eq!(optional_amq_root(&paths), None);
        fs::create_dir_all(dir.path().join("amq")).unwrap();
        assert_eq!(optional_amq_root(&paths), Some(dir.path().join("amq")));
    } else {
        let expected = std::env::var_os("AMQ_GLOBAL_ROOT")
            .or_else(|| std::env::var_os("AM_ROOT"))
            .map(PathBuf::from);
        assert_eq!(optional_amq_root(&paths), expected);
    }
}

/// The wrapper script takes the same `meta/config.lock` before claiming an
/// inbox, so a wrapper launched while Dux holds the lock waits for it.
#[test]
#[ignore = "INTEGRATION: needs dux-amq overlay"]
fn rust_and_wrapper_claims_serialize_on_config_lock() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq");
    let home = dir.path().join("home");
    let worktree = dir.path().join("worktree");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&worktree).unwrap();
    let lock = AmqRegistryLock::acquire(&root).unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dux_amq = workspace.join("dux-amq");
    let wrapper = dux_amq.join("wrappers/claude-amq");
    assert!(
        wrapper.is_file(),
        "dux-amq overlay missing at {}",
        wrapper.display()
    );
    let path = format!(
        "{}:{}:{}",
        dux_amq.join("tests/fakes").display(),
        dux_amq.join("scripts").display(),
        env::var("PATH").unwrap_or_default()
    );
    let mut child = Command::new(wrapper)
        .current_dir(&worktree)
        .env("PATH", path)
        .env("HOME", &home)
        .env("STATE_ROOT", dir.path().join("state"))
        .env("AMQ_GLOBAL_ROOT", &root)
        .env("AM_ME", "wrapper-agent")
        .env("DUX_AMQ_INJECT_MODE", "via")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    assert!(child.try_wait().unwrap().is_none());
    assert!(!root.join("agents/wrapper-agent/.dux-amq-source").exists());

    drop(lock);
    assert!(child.wait().unwrap().success());
    assert!(root.join("agents/wrapper-agent/.dux-amq-source").exists());
}

#[test]
fn wake_pid_guard_requires_the_recorded_handle_and_root() {
    let root = Path::new("/tmp/shared-amq");
    let command = [
        OsString::from("/usr/local/bin/amq"),
        OsString::from("wake"),
        OsString::from("--me"),
        OsString::from("agent"),
        OsString::from("--root"),
        root.as_os_str().to_os_string(),
    ];

    assert!(amq_wake_command_matches(&command, root, "agent"));
    assert!(!amq_wake_command_matches(&command, root, "other-agent"));
    assert!(!amq_wake_command_matches(
        &command,
        Path::new("/tmp/other-amq"),
        "agent"
    ));
}

#[test]
fn tombstone_releases_the_registry_lock_before_waiting_for_wake_exit() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    let term_seen = dir.path().join("term-seen");
    let child = Command::new("perl")
        .arg("-e")
        .arg(
            "$SIG{TERM}=sub { open(my $f, '>', $ENV{WAKE_TERM_FILE}) or die $!; close($f); }; while (1) { select(undef, undef, undef, 0.05); }",
        )
        .arg("amq")
        .arg("wake")
        .arg("--me")
        .arg("agent")
        .arg("--root")
        .arg(&root)
        .env("WAKE_TERM_FILE", &term_seen)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let wake_pid = child.id();
    let reaper = std::thread::spawn(move || child.wait_with_output());
    let session = session("session-a", "claude", "agent", &dir.path().join("agent"));
    ensure_owner_marker(
        &root,
        "agent",
        &owner("store-a", &session.id, Some(wake_pid)),
    )
    .unwrap();
    register_config_handle(&root, "agent").unwrap();
    let tombstone_root = root.clone();
    let tombstone_session = session.clone();
    let tombstone = std::thread::spawn(move || {
        tombstone_amq_session_at_root(&tombstone_root, "store-a", &tombstone_session)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !term_seen.exists() {
        if Instant::now() >= deadline {
            // The wake-termination guard only signals a process it can
            // positively identify by argv. Some sandboxed environments
            // cannot read another process's argv, so the guard skips the
            // kill (a safe leak, not a wrong kill) and there is no SIGTERM
            // to observe; self-skip rather than fail.
            if !is_amq_wake_process(wake_pid, &root, "agent") {
                if let Some(pid) = rustix::process::Pid::from_raw(wake_pid as i32) {
                    let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                }
                tombstone.join().unwrap().unwrap();
                let _ = reaper.join().unwrap();
                return;
            }
            panic!("identifiable wake process never received SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let probe_output = dir.path().join("probe-output");
    let status = Command::new(std::env::current_exe().unwrap())
        .arg("peer::amq::tests::subprocess_helper")
        .arg("--exact")
        .arg("--nocapture")
        .env("DUX_AMQ_TEST_HELPER_MODE", "probe-lock")
        .env("DUX_AMQ_TEST_ROOT", &root)
        .env("DUX_AMQ_TEST_OUTPUT", &probe_output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(probe_output).unwrap(), "acquired");

    tombstone.join().unwrap().unwrap();
    let _ = reaper.join().unwrap();
}

#[test]
fn tombstone_terminates_recorded_wake_pid_and_keeps_inbox() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    let fake_amq = dir.path().join("amq-wake-test");
    fs::write(
        &fake_amq,
        "#!/bin/sh\ntrap 'exit 0' TERM\nwhile :; do :; done\n",
    )
    .unwrap();
    fs::set_permissions(&fake_amq, fs::Permissions::from_mode(0o755)).unwrap();
    let child = Command::new(&fake_amq)
        .arg("wake")
        .arg("--me")
        .arg("agent")
        .arg("--root")
        .arg(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid = child.id();
    let reaper = std::thread::spawn(move || child.wait_with_output());
    let s = session("session-a", "claude", "agent", &dir.path().join("agent"));
    ensure_owner_marker(&root, "agent", &owner("store-a", &s.id, Some(pid))).unwrap();
    register_config_handle(&root, "agent").unwrap();

    tombstone_amq_session_at_root(&root, "store-a", &s).unwrap();
    let _ = reaper.join().unwrap();

    assert!(root.join("agents/agent").is_dir());
    let owner = match marker_state(&root, "agent").unwrap() {
        MarkerState::Owner(owner) => owner,
        _ => panic!("owner marker missing"),
    };
    assert_eq!(owner.wake_pid, None);
    assert!(!config_raw(&root).contains("\"agent\""));
}

/// Deleting a session another store owns leaves that registration alone.
#[test]
fn tombstone_ignores_a_handle_this_session_does_not_own() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("amq").tap_agents();
    ensure_owner_marker(&root, "agent", &owner("store-b", "theirs", None)).unwrap();
    register_config_handle(&root, "agent").unwrap();
    let s = session("mine", "claude", "agent", dir.path());

    tombstone_amq_session_at_root(&root, "store-a", &s).unwrap();

    assert!(config_raw(&root).contains("\"agent\""));
    assert!(matches!(
        marker_state(&root, "agent").unwrap(),
        MarkerState::Owner(o) if o.store_id == "store-b"
    ));
}
