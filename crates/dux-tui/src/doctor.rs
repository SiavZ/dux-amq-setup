//! `dux doctor`: an operator's diagnostic dump (port of fork c6426735, with the
//! 55dba0f7 hardening: one merged JSON object, anonymized Rust fields, capped
//! orphan details, a read-only database open).
//!
//! Most of the report comes from the `dux-amq-doctor` shell script (versions,
//! disk, AMQ, symlinks, kernel, recent errors), which the dux-amq overlay
//! installs. This wrapper gives it a discoverable entry point and adds the one
//! section only Rust can do without extra tools: the sessions database, opened
//! read-only and counted with the same model code dux uses. Without the script
//! it still prints that section.
//!
//! Read-only on purpose, including no lock: operators run it precisely when
//! dux is misbehaving, often while a TUI holds the lock.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use dux_core::config::DuxPaths;
use dux_core::model::SessionStatus;
use dux_core::storage::SessionStore;

/// Most orphaned sessions listed individually; the count is always complete.
const ORPHANED_SESSIONS_LIMIT: usize = 10;

/// Where the shell half is looked for: `DUX_AMQ_DOCTOR_BIN`, then
/// `dux-amq-doctor` on `PATH`. `None` prints the Rust section alone.
fn resolve_doctor_script() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("DUX_AMQ_DOCTOR_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join("dux-amq-doctor"))
        .find(|candidate| candidate.is_file())
}

/// Entry point for `dux doctor [--json] [--anonymize]`.
pub fn run_doctor(paths: &DuxPaths, json: bool, anonymize: bool) -> Result<()> {
    let script = resolve_doctor_script();
    if json {
        let mut bash_json = serde_json::json!({});
        let mut bash_ran = false;
        if let Some(script) = &script {
            let mut cmd = Command::new(script);
            cmd.arg("--json");
            if anonymize {
                cmd.arg("--anonymize");
            }
            match cmd.output() {
                Ok(output) if output.status.success() => {
                    bash_json = serde_json::from_slice(&output.stdout)
                        .with_context(|| format!("{} emitted invalid JSON", script.display()))?;
                    bash_ran = true;
                }
                Ok(output) => eprintln!(
                    "warning: dux-amq-doctor exited with {}; emitting Rust-only JSON",
                    output.status.code().unwrap_or(-1),
                ),
                Err(err) => eprintln!(
                    "warning: failed to spawn {}: {err}; emitting Rust-only JSON",
                    script.display()
                ),
            }
        }
        let snap = collect_sessions_snapshot(paths);
        let rust_json = build_rust_section_json(paths, &snap, anonymize);
        println!("{}", merge_doctor_json(bash_json, rust_json, bash_ran)?);
        return Ok(());
    }

    // Text mode lets the script own stdout so its TTY color detection works.
    let bash_ran = match &script {
        Some(script) => {
            let mut cmd = Command::new(script);
            if anonymize {
                cmd.arg("--anonymize");
            }
            match cmd.status() {
                Ok(status) if status.success() => true,
                Ok(status) => {
                    eprintln!(
                        "warning: dux-amq-doctor exited with {}; falling back to Rust-only output",
                        status.code().unwrap_or(-1),
                    );
                    false
                }
                Err(err) => {
                    eprintln!("warning: failed to spawn {}: {err}", script.display());
                    false
                }
            }
        }
        None => false,
    };
    let snap = collect_sessions_snapshot(paths);
    print!(
        "{}",
        render_rust_section_text(paths, &snap, bash_ran, anonymize)
    );
    Ok(())
}

/// One JSON object: the script's sections plus `sessions_db_rust`, so a
/// consumer never has to `jq -s add` two documents.
fn merge_doctor_json(
    mut bash: serde_json::Value,
    rust: serde_json::Value,
    bash_ran: bool,
) -> Result<serde_json::Value> {
    if !bash_ran {
        bash = serde_json::json!({});
    }
    let bash_obj = bash
        .as_object_mut()
        .context("doctor JSON root must be an object")?;
    let rust_obj = rust
        .as_object()
        .context("Rust doctor JSON root must be an object")?;
    for (key, value) in rust_obj {
        bash_obj.insert(key.clone(), value.clone());
    }
    Ok(bash)
}

fn render_rust_section_text(
    paths: &DuxPaths,
    snap: &SessionsSnapshot,
    bash_ran: bool,
    anonymize: bool,
) -> String {
    let mut out = String::new();
    if !bash_ran {
        out.push_str(
            "(dux-amq-doctor unavailable or failed; emitting Rust-only diagnostics. \
             Install the dux-amq overlay or run dux-amq-doctor directly for the full report.)\n",
        );
    }
    out.push_str("\n== Sessions DB (Rust-side) ==\n");
    out.push_str(&format!(
        "{:<16} {}\n",
        "path:",
        doctor_db_path(paths, anonymize)
    ));
    out.push_str(&format!("{:<16} {}\n", "integrity:", snap.integrity));
    out.push_str(&format!("{:<16} {}\n", "active:", snap.active));
    out.push_str(&format!("{:<16} {}\n", "detached:", snap.detached));
    out.push_str(&format!("{:<16} {}\n", "exited:", snap.exited));
    out.push_str(&format!(
        "{:<16} {}\n",
        "orphaned:", snap.orphaned_worktrees
    ));
    append_orphaned_sessions_text(&mut out, snap, anonymize);
    out
}

/// The identifying fields of orphan `index`, replaced by placeholders under
/// `--anonymize` so a pasted report names no branch, path or provider.
fn orphan_fields(session: &OrphanedSessionSnapshot, index: usize, anonymize: bool) -> [String; 4] {
    if anonymize {
        let n = index + 1;
        [
            format!("session-{n}"),
            format!("provider-{n}"),
            format!("branch-{n}"),
            format!("/WT/session-{n}"),
        ]
    } else {
        [
            session.id.clone(),
            session.provider.clone(),
            session.branch.clone(),
            session.worktree_path.clone(),
        ]
    }
}

fn append_orphaned_sessions_text(out: &mut String, snap: &SessionsSnapshot, anonymize: bool) {
    if snap.orphaned_sessions.is_empty() {
        return;
    }
    let shown = snap.orphaned_sessions.len();
    let summary = if shown < snap.orphaned_worktrees {
        format!(
            "first {shown} of {} (limit {ORPHANED_SESSIONS_LIMIT})",
            snap.orphaned_worktrees
        )
    } else {
        shown.to_string()
    };
    out.push_str(&format!("{:<16} {summary}\n", "orphaned_list:"));
    for (index, session) in snap.orphaned_sessions.iter().enumerate() {
        let [id, provider, branch, worktree_path] = orphan_fields(session, index, anonymize);
        out.push_str(&format!(
            "  - id={id} provider={provider} branch={branch} state={} worktree_path={worktree_path}\n",
            session.state
        ));
    }
}

fn build_rust_section_json(
    paths: &DuxPaths,
    snap: &SessionsSnapshot,
    anonymize: bool,
) -> serde_json::Value {
    let orphaned_sessions: Vec<serde_json::Value> = snap
        .orphaned_sessions
        .iter()
        .enumerate()
        .map(|(index, session)| {
            let [id, provider, branch, worktree_path] = orphan_fields(session, index, anonymize);
            serde_json::json!({
                "id": id,
                "provider": provider,
                "branch": branch,
                "state": session.state,
                "worktree_path": worktree_path,
            })
        })
        .collect();
    serde_json::json!({
        "sessions_db_rust": {
            "path": doctor_db_path(paths, anonymize),
            "integrity": snap.integrity,
            "active": snap.active,
            "detached": snap.detached,
            "exited": snap.exited,
            "orphaned_worktrees": snap.orphaned_worktrees,
            "orphaned_sessions_limit": ORPHANED_SESSIONS_LIMIT,
            "orphaned_sessions_truncated": snap.orphaned_sessions.len() < snap.orphaned_worktrees,
            "orphaned_sessions": orphaned_sessions,
        }
    })
}

fn doctor_db_path(paths: &DuxPaths, anonymize: bool) -> String {
    if anonymize {
        "/DUX/sessions.sqlite3".to_string()
    } else {
        paths.sessions_db_path.display().to_string()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OrphanedSessionSnapshot {
    id: String,
    provider: String,
    branch: String,
    state: String,
    worktree_path: String,
}

struct SessionsSnapshot {
    integrity: String,
    active: usize,
    detached: usize,
    exited: usize,
    /// Sessions whose working directory no longer exists on disk.
    orphaned_worktrees: usize,
    orphaned_sessions: Vec<OrphanedSessionSnapshot>,
}

impl SessionsSnapshot {
    fn empty(integrity: String) -> Self {
        Self {
            integrity,
            active: 0,
            detached: 0,
            exited: 0,
            orphaned_worktrees: 0,
            orphaned_sessions: Vec::new(),
        }
    }
}

fn collect_sessions_snapshot(paths: &DuxPaths) -> SessionsSnapshot {
    if !paths.sessions_db_path.exists() {
        return SessionsSnapshot::empty("absent".to_string());
    }
    let store = match SessionStore::open_read_only(&paths.sessions_db_path) {
        Ok(store) => store,
        Err(err) => return SessionsSnapshot::empty(format!("error: {err:#}")),
    };
    let sessions = match store.load_sessions() {
        Ok(sessions) => sessions,
        // A database from an older dux lacks columns this build reads, and a
        // read-only open cannot migrate it. Say so instead of reporting zero.
        Err(err) => {
            return SessionsSnapshot::empty(format!(
                "ok (sessions unreadable without a migration; start dux once: {err:#})"
            ));
        }
    };
    let mut snap = SessionsSnapshot::empty("ok".to_string());
    for session in &sessions {
        match session.status {
            SessionStatus::Active => snap.active += 1,
            SessionStatus::Detached => snap.detached += 1,
            SessionStatus::Exited => snap.exited += 1,
        }
        if !Path::new(session.directory()).exists() {
            snap.orphaned_worktrees += 1;
            if snap.orphaned_sessions.len() < ORPHANED_SESSIONS_LIMIT {
                snap.orphaned_sessions.push(OrphanedSessionSnapshot {
                    id: session.id.clone(),
                    provider: session.provider.as_str().to_string(),
                    branch: session.branch_name().unwrap_or_default().to_string(),
                    state: session.status.as_str().to_string(),
                    worktree_path: session.directory().to_string(),
                });
            }
        }
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn paths_in(root: &Path) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.to_path_buf(),
        }
    }

    fn session_at(id: &str, dir: &Path, status: SessionStatus) -> dux_core::model::AgentSession {
        let dir = dir.to_string_lossy().to_string();
        let now = chrono::Utc::now();
        dux_core::model::AgentSession {
            id: id.to_string(),
            agent_handle: id.to_string(),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: format!("{id}-slot"),
            provider: dux_core::model::ProviderKind::from_str("claude"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: false,
            status,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: dux_core::model::AgentWorkspace::Managed(
                dux_core::model::ManagedWorkspace {
                    project_id: "p".to_string(),
                    project_path: None,
                    source_branch: "main".to_string(),
                    branch_name: format!("b-{id}"),
                    initial_branch: format!("b-{id}"),
                    branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                    worktree_path: dir,
                },
            ),
        }
    }

    #[test]
    fn doctor_snapshot_handles_missing_db() {
        let tmp = TempDir::new().unwrap();
        let snap = collect_sessions_snapshot(&paths_in(tmp.path()));
        assert_eq!(snap.integrity, "absent");
        assert_eq!(snap.active, 0);
        assert_eq!(snap.orphaned_worktrees, 0);
        assert!(snap.orphaned_sessions.is_empty());
    }

    /// A diagnostic must not change what it diagnoses: no migration, no
    /// journal switch, not one byte.
    #[test]
    fn doctor_snapshot_does_not_migrate_or_change_database_bytes() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        {
            let conn = rusqlite::Connection::open(&paths.sessions_db_path).unwrap();
            conn.execute_batch(
                "create table diagnostic_sentinel(value text); pragma user_version = 0;",
            )
            .unwrap();
        }
        let before = std::fs::read(&paths.sessions_db_path).unwrap();
        let snap = collect_sessions_snapshot(&paths);
        let after = std::fs::read(&paths.sessions_db_path).unwrap();
        assert!(snap.integrity.starts_with("ok"), "{}", snap.integrity);
        assert_eq!(after, before);

        let conn = rusqlite::Connection::open(&paths.sessions_db_path).unwrap();
        let migrated: i64 = conn
            .query_row(
                "select count(*) from sqlite_master where name='agent_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(migrated, 0);
        assert!(!tmp.path().join("sessions.sqlite3-wal").exists());
    }

    #[test]
    fn doctor_json_merges_to_one_object_and_anonymizes_rust_fields() {
        let paths = DuxPaths {
            root: PathBuf::from("/secret/home/.config/dux"),
            config_path: PathBuf::from("/secret/home/.config/dux/config.toml"),
            sessions_db_path: PathBuf::from("/secret/home/.config/dux/sessions.sqlite3"),
            worktrees_root: PathBuf::from("/secret/home/worktrees"),
            lock_path: PathBuf::from("/secret/home/.config/dux/dux.lock"),
        };
        let snap = SessionsSnapshot {
            orphaned_worktrees: 1,
            orphaned_sessions: vec![OrphanedSessionSnapshot {
                id: "customer-session-id".to_string(),
                provider: "secret-provider".to_string(),
                branch: "customer/feature".to_string(),
                state: "detached".to_string(),
                worktree_path: "/secret/home/worktrees/customer-feature".to_string(),
            }],
            ..SessionsSnapshot::empty("ok".to_string())
        };
        let merged = merge_doctor_json(
            serde_json::json!({"versions": {"dux": "test"}}),
            build_rust_section_json(&paths, &snap, true),
            true,
        )
        .unwrap();
        let encoded = serde_json::to_string(&merged).unwrap();
        assert_eq!(merged["versions"]["dux"], "test");
        assert_eq!(merged["sessions_db_rust"]["path"], "/DUX/sessions.sqlite3");
        for secret in [
            "/secret/home",
            "customer-session-id",
            "secret-provider",
            "customer/feature",
        ] {
            assert!(!encoded.contains(secret), "leaked {secret}: {encoded}");
        }
        assert!(encoded.contains("session-1"));

        // Without the script's half the object is the Rust section alone.
        let alone = merge_doctor_json(
            serde_json::json!({"stale": true}),
            build_rust_section_json(&paths, &snap, false),
            false,
        )
        .unwrap();
        assert!(alone.get("stale").is_none());
        assert!(alone.get("sessions_db_rust").is_some());
    }

    #[test]
    fn doctor_snapshot_counts_by_status_and_orphans() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        let live = tmp.path().join("worktrees").join("live-1");
        std::fs::create_dir_all(&live).unwrap();
        let orphan = tmp.path().join("worktrees").join("missing-x1");
        {
            let store = SessionStore::open(&paths.sessions_db_path).unwrap();
            for session in [
                session_at("d1", &live, SessionStatus::Detached),
                session_at("d2", &live, SessionStatus::Detached),
                session_at("a1", &live, SessionStatus::Active),
                session_at("x1", &orphan, SessionStatus::Exited),
            ] {
                store.upsert_session(&session).unwrap();
            }
        }

        let snap = collect_sessions_snapshot(&paths);
        assert_eq!(snap.integrity, "ok");
        assert_eq!(snap.active, 1);
        assert_eq!(snap.detached, 2);
        assert_eq!(snap.exited, 1);
        assert_eq!(snap.orphaned_worktrees, 1);
        let orphan_path = orphan.to_string_lossy().to_string();
        assert_eq!(
            snap.orphaned_sessions,
            vec![OrphanedSessionSnapshot {
                id: "x1".to_string(),
                provider: "claude".to_string(),
                branch: "b-x1".to_string(),
                state: "exited".to_string(),
                worktree_path: orphan_path.clone(),
            }]
        );

        let text = render_rust_section_text(&paths, &snap, true, false);
        assert!(text.contains("orphaned_list:"));
        assert!(text.contains(&format!(
            "id=x1 provider=claude branch=b-x1 state=exited worktree_path={orphan_path}"
        )));
        let json = build_rust_section_json(&paths, &snap, false);
        assert_eq!(json["sessions_db_rust"]["orphaned_worktrees"], 1);
        assert_eq!(
            json["sessions_db_rust"]["orphaned_sessions_truncated"],
            false
        );
        assert_eq!(json["sessions_db_rust"]["orphaned_sessions"][0]["id"], "x1");
    }

    #[test]
    fn doctor_snapshot_caps_orphaned_session_details() {
        let tmp = TempDir::new().unwrap();
        let paths = paths_in(tmp.path());
        {
            let store = SessionStore::open(&paths.sessions_db_path).unwrap();
            for n in 0..(ORPHANED_SESSIONS_LIMIT + 3) {
                let dir = tmp.path().join(format!("gone-{n}"));
                store
                    .upsert_session(&session_at(&format!("s{n}"), &dir, SessionStatus::Detached))
                    .unwrap();
            }
        }
        let snap = collect_sessions_snapshot(&paths);
        assert_eq!(snap.orphaned_worktrees, ORPHANED_SESSIONS_LIMIT + 3);
        assert_eq!(snap.orphaned_sessions.len(), ORPHANED_SESSIONS_LIMIT);
        let text = render_rust_section_text(&paths, &snap, true, false);
        assert!(text.contains(&format!(
            "first {ORPHANED_SESSIONS_LIMIT} of {} (limit {ORPHANED_SESSIONS_LIMIT})",
            ORPHANED_SESSIONS_LIMIT + 3
        )));
        let json = build_rust_section_json(&paths, &snap, false);
        assert_eq!(
            json["sessions_db_rust"]["orphaned_sessions_truncated"],
            true
        );
    }
}
