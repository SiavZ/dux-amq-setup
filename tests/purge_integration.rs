//! Integration tests for `dux::purge` — audit02 Phase 10 (P0-J).
//!
//! These tests fabricate a self-contained dux installation under a
//! tempdir (sqlite, worktree, fake provider chat dirs, fake AMQ inbox,
//! fake JSON Lines log) and exercise `build_plan` + `execute` end-to-end
//! across the four scenarios listed in the plan:
//!
//!   1. `purge_removes_all_known_categories` — happy path.
//!   2. `purge_dry_run_changes_nothing`     — every step must honor `--dry-run`.
//!   3. `purge_aborts_on_wrong_confirmation` — confirm phrase mismatch.
//!   4. `purge_with_unknown_target_returns_error_not_panic` — target not found.
//!
//! Plus three extras that proved load-bearing while writing the module:
//!
//!   5. `purge_redacts_log_records_for_session` — log scoping correctness.
//!   6. `purge_skips_missing_provider_dirs`     — non-error skip path.
//!   7. `purge_executes_in_documented_order`     — worktree before sqlite.
//!
//! Tests construct `DuxPaths` and `PurgeConfig` by hand rather than going
//! through `DuxPaths::discover()` so each test is hermetic and parallel-safe.

use std::fs;
use std::io::Write;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use chrono::Utc;
use dux::config::DuxPaths;
use dux::model::{AgentSession, ProviderKind, SessionState};
use dux::purge::{
    self, PurgeConfig, PurgeItem, PurgeOutcome, SharedPurgeMode, build_plan, build_plans_for_all,
    build_plans_for_target, confirm_with_reader, execute, plan_for_session,
};
use dux::purge_encoding;
use dux::storage::{SessionStore, load_or_create_store_id, load_store_id};

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct PurgeHarness {
    tmp: tempfile::TempDir,
    paths: DuxPaths,
    config: PurgeConfig,
    storage: SessionStore,
    session: AgentSession,
    /// Path to the fake `<claude_root>/projects/<encoded>` dir we
    /// expect to be deleted.
    claude_dir: PathBuf,
    /// Path to the fake `<codex_root>/projects/<encoded>` dir we
    /// expect to be deleted.
    codex_dir: PathBuf,
    /// Path to the fake AMQ inbox we expect to be deleted.
    amq_inbox: PathBuf,
    /// Path to the live `dux.log` we expect to be redacted.
    log_path: PathBuf,
}

impl PurgeHarness {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        // Create a worktrees root that lives inside `root` so the
        // session's worktree path canonicalizes within the harness.
        let worktrees_root = root.join("worktrees");
        fs::create_dir_all(&worktrees_root).expect("worktrees root");

        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: worktrees_root.clone(),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };

        // Build provider/AMQ roots inside the tmp so we never touch
        // the real /data/state/*.
        let claude_root = root.join("claude");
        let codex_root = root.join("codex");
        let amq_root = root.join("amq");
        let config = PurgeConfig {
            provider_data_dirs: vec![
                ("claude".to_string(), claude_root.clone()),
                ("codex".to_string(), codex_root.clone()),
            ],
            amq_root: amq_root.clone(),
            log_path: root.join("dux.log"),
            registered_project_paths: Vec::new(),
        };

        // Pick a worktree path. Use a constant suffix so encoded path
        // is stable across runs.
        let worktree = worktrees_root.join("audit02-x");
        fs::create_dir_all(&worktree).expect("worktree");
        // Drop a marker file so an empty-dir-quick-skip wouldn't fool
        // the test.
        fs::write(worktree.join("HEAD"), b"ref: refs/heads/audit02-x\n").expect("HEAD");

        // Encode the worktree path the way Claude Code would, then
        // pre-create the matching provider dirs with a marker file.
        let encoded =
            purge_encoding::encode_str(&worktree.to_string_lossy()).expect("encode worktree");
        let claude_dir = claude_root.join("projects").join(&encoded);
        let codex_dir = codex_root.join("projects").join(&encoded);
        fs::create_dir_all(&claude_dir).expect("claude dir");
        fs::create_dir_all(&codex_dir).expect("codex dir");
        fs::write(claude_dir.join("history.jsonl"), b"chat\n").expect("claude history");
        fs::write(codex_dir.join("history.jsonl"), b"chat\n").expect("codex history");

        // Fake AMQ inbox under <amq_root>/agents/<agent_handle>/inbox.
        let amq_inbox = amq_root.join("agents").join("sid-target");
        fs::create_dir_all(amq_inbox.join("inbox")).expect("amq inbox");
        fs::write(amq_inbox.join("inbox/00001.json"), b"{}\n").expect("amq msg");
        let store_id = load_or_create_store_id(&root).expect("store id");
        fs::write(
            amq_inbox.join(".dux-amq-source"),
            format!(r#"{{"store_id":"{store_id}","session_id":"sid-target"}}"#),
        )
        .expect("owner marker");
        fs::create_dir_all(amq_root.join("meta")).expect("amq meta");
        fs::write(
            amq_root.join("meta/config.json"),
            r#"{"agents":["sid-target"]}"#,
        )
        .expect("amq config");

        // Fake live JSON Lines log file. Note: the production code
        // matches files named `dux.log*`, so the live file must be
        // `dux.log` (not the rotated `dux.log.YYYY-MM-DD` form).
        let log_path = root.join("dux.log");
        let mut log = fs::File::create(&log_path).expect("log create");
        // Three lines: one matching session, one different session, one
        // line missing session_id entirely. Only the first should be
        // redacted.
        writeln!(
            log,
            r#"{{"target":"dux::probe","fields":{{"session_id":"sid-target","msg":"x"}}}}"#
        )
        .unwrap();
        writeln!(
            log,
            r#"{{"target":"dux::probe","fields":{{"session_id":"sid-other","msg":"y"}}}}"#
        )
        .unwrap();
        writeln!(
            log,
            r#"{{"target":"dux::probe","fields":{{"msg":"no sid"}}}}"#
        )
        .unwrap();
        log.sync_all().unwrap();
        drop(log);

        // Open the sqlite store and insert the target session.
        let storage = SessionStore::open(&paths.sessions_db_path).expect("store");
        let now = Utc::now();
        let session = AgentSession {
            id: "sid-target".to_string(),
            project_id: "proj".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "audit02-x".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            agent_handle: "sid-target".to_string(),
            shared_workspace: false,
            deleted_at: None,
            title: None,
            started_providers: Vec::new(),
            provider_session_ids: Default::default(),
            state: SessionState::Created { created_at: now },
            settings: dux::model::SessionSettings::default(),
            created_at: now,
            updated_at: now,
        };
        storage.upsert_session(&session).expect("upsert");

        Self {
            tmp,
            paths,
            config,
            storage,
            session,
            claude_dir,
            codex_dir,
            amq_inbox,
            log_path,
        }
    }

    fn worktree(&self) -> PathBuf {
        PathBuf::from(&self.session.worktree_path)
    }
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn write_owned_amq_marker(h: &PurgeHarness, session: &AgentSession, handles: &[&str]) {
    let inbox = h
        .config
        .amq_root
        .join("agents")
        .join(session.agent_handle());
    fs::create_dir_all(inbox.join("inbox")).unwrap();
    fs::write(
        inbox.join(".dux-amq-source"),
        format!(
            r#"{{"store_id":"{}","session_id":"{}"}}"#,
            load_store_id(&h.paths.root).unwrap(),
            session.id
        ),
    )
    .unwrap();
    let agents = handles
        .iter()
        .map(|handle| format!(r#""{handle}""#))
        .collect::<Vec<_>>()
        .join(",");
    fs::write(
        h.config.amq_root.join("meta/config.json"),
        format!(r#"{{"agents":[{agents}]}}"#),
    )
    .unwrap();
}

fn count_target_records(log_text: &str, target_sid: &str) -> usize {
    log_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let v: serde_json::Value = match serde_json::from_str(l) {
                Ok(v) => v,
                Err(_) => return false,
            };
            v.get("fields")
                .and_then(|f| f.get("session_id"))
                .and_then(|s| s.as_str())
                .map(|s| s == target_sid)
                .unwrap_or(false)
        })
        .count()
}

fn count_redacted_records(log_text: &str) -> usize {
    log_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let v: serde_json::Value = match serde_json::from_str(l) {
                Ok(v) => v,
                Err(_) => return false,
            };
            v.get("fields")
                .and_then(|f| f.get("redacted"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        })
        .count()
}

fn assert_failure_retains_row_and_retry_succeeds(
    h: &PurgeHarness,
    item: PurgeItem,
    execution_paths: &DuxPaths,
    category: &str,
) {
    let failed_plan = dux::purge::PurgePlan {
        session_id: h.session.id.clone(),
        branch: h.session.branch_name.clone(),
        items: vec![item, PurgeItem::SqliteRow],
    };
    let failed =
        execute(&failed_plan, &h.storage, execution_paths, &h.config, false).expect("execute");
    assert!(failed.had_errors(), "{category} fault did not fail");
    assert!(matches!(
        failed.entries.last(),
        Some((PurgeItem::SqliteRow, PurgeOutcome::Skipped(reason)))
            if reason.contains("retained for retry")
    ));
    assert_eq!(
        h.storage.load_sessions().expect("load after failure").len(),
        1,
        "{category} failure deleted the durable session identity"
    );

    let retry = build_plan(&h.storage, &h.paths, &h.config, &h.session.id)
        .expect("retained row must remain resolvable");
    let retried = execute(&retry, &h.storage, &h.paths, &h.config, false).expect("retry execute");
    assert!(
        !retried.had_errors(),
        "{category} retry: {}",
        retried.summary()
    );
    assert!(
        h.storage
            .load_sessions()
            .expect("load after retry")
            .is_empty(),
        "{category} retry did not delete the completed session row"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn purge_removes_all_known_categories() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");

    let report = execute(&plan, &h.storage, &h.paths, &h.config, false).expect("execute");
    assert!(!report.had_errors(), "summary:\n{}", report.summary());

    // Worktree gone.
    assert!(
        !h.worktree().exists(),
        "worktree should be removed: {}",
        h.worktree().display()
    );
    // Provider dirs gone.
    assert!(
        !h.claude_dir.exists(),
        "claude dir should be removed: {}",
        h.claude_dir.display()
    );
    assert!(
        !h.codex_dir.exists(),
        "codex dir should be removed: {}",
        h.codex_dir.display()
    );
    // Provider parent (`<root>/claude/projects/`) preserved — we
    // never delete recursively above the encoded dir.
    assert!(h.claude_dir.parent().expect("parent").exists());
    // AMQ inbox gone, but the parent `agents/` dir preserved.
    assert!(!h.amq_inbox.exists());
    assert!(h.amq_inbox.parent().expect("agents parent").exists());
    // Sqlite row gone.
    let surviving = h.storage.load_sessions().expect("load");
    assert!(
        surviving.iter().all(|s| s.id != h.session.id),
        "session row should be removed"
    );
    // Log: target records redacted, others untouched.
    let log_text = read_log(&h.log_path);
    assert_eq!(
        count_target_records(&log_text, "sid-target"),
        0,
        "target session_id should not appear in log:\n{log_text}"
    );
    // Other-session record still there.
    assert_eq!(count_target_records(&log_text, "sid-other"), 1);
    // We replaced the matching record's fields with {redacted:true}.
    assert!(count_redacted_records(&log_text) >= 1);
    // The harness keeps the tempdir alive.
    let _keep = h.tmp;
}

#[test]
fn purge_dry_run_changes_nothing() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");
    let log_before = read_log(&h.log_path);

    let report = execute(&plan, &h.storage, &h.paths, &h.config, true).expect("execute");
    assert!(!report.had_errors());
    assert!(report.dry_run);
    assert!(
        report
            .entries
            .iter()
            .all(|(_, o)| matches!(o, PurgeOutcome::DryRun)),
        "every step must report DryRun on --dry-run; got {:?}",
        report.entries
    );

    // Nothing on disk should have moved.
    assert!(h.worktree().exists());
    assert!(h.claude_dir.exists());
    assert!(h.codex_dir.exists());
    assert!(h.amq_inbox.exists());
    let log_after = read_log(&h.log_path);
    assert_eq!(log_before, log_after, "log file must be byte-identical");
    let surviving = h.storage.load_sessions().expect("load");
    assert_eq!(surviving.len(), 1);
    let _keep = h.tmp;
}

#[test]
fn purge_aborts_on_wrong_confirmation() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");

    // Wrong branch in the phrase.
    let mut wrong = std::io::Cursor::new(b"PURGE wrong\n".to_vec());
    assert!(!confirm_with_reader(&plan, &mut wrong).unwrap());

    // Different verb.
    let mut wrong = std::io::Cursor::new(b"DELETE audit02-x\n".to_vec());
    assert!(!confirm_with_reader(&plan, &mut wrong).unwrap());

    // Empty input.
    let mut empty = std::io::Cursor::new(b"\n".to_vec());
    assert!(!confirm_with_reader(&plan, &mut empty).unwrap());

    // Verify nothing was deleted (we never called `execute`).
    assert!(h.worktree().exists());
    assert!(h.claude_dir.exists());
    assert!(h.amq_inbox.exists());
    let _keep = h.tmp;
}

#[test]
fn purge_with_unknown_target_returns_error_not_panic() {
    let h = PurgeHarness::new();
    let result = build_plan(&h.storage, &h.paths, &h.config, "no-such-session");
    let err = result.expect_err("should error on unknown target");
    let msg = err.to_string();
    assert!(
        msg.contains("no session found"),
        "error message should explain: got {msg}"
    );

    // Same for an obviously-malformed input.
    let result = build_plan(&h.storage, &h.paths, &h.config, "");
    assert!(result.is_err());
    let _keep = h.tmp;
}

#[test]
fn purge_redacts_log_records_for_session() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");
    let report = execute(&plan, &h.storage, &h.paths, &h.config, false).expect("execute");
    assert!(!report.had_errors(), "{}", report.summary());

    let text = read_log(&h.log_path);
    // Every line should still parse as JSON.
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "line broke JSON shape: {line}");
    }
    // Target session's content is gone.
    assert!(!text.contains("\"sid-target\""));
    // Other session's content survives.
    assert!(text.contains("\"sid-other\""));
    // The redacted marker is present.
    assert!(text.contains("\"redacted\":true"));
    let _keep = h.tmp;
}

#[test]
fn purge_skips_missing_provider_dirs() {
    let h = PurgeHarness::new();
    // Pre-delete the codex dir so the cascade has to skip it cleanly.
    fs::remove_dir_all(&h.codex_dir).expect("pre-delete");
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");
    let report = execute(&plan, &h.storage, &h.paths, &h.config, false).expect("execute");
    assert!(!report.had_errors(), "{}", report.summary());

    // The codex outcome should be Skipped, not Error.
    let codex_outcome = report
        .entries
        .iter()
        .find(|(item, _)| {
            matches!(
                item,
                PurgeItem::ProviderDir {
                    provider: "codex",
                    ..
                }
            )
        })
        .map(|(_, o)| o.clone())
        .expect("codex entry");
    assert!(
        matches!(codex_outcome, PurgeOutcome::Skipped(_)),
        "expected codex to be Skipped, got {codex_outcome:?}"
    );
    let _keep = h.tmp;
}

#[test]
fn purge_executes_in_documented_order() {
    let h = PurgeHarness::new();
    let plan = plan_for_session(&h.session, &h.paths, &h.config).expect("plan");

    // Order: Worktree, ProviderDir(claude), ProviderDir(codex),
    // AmqInbox, LogScopedRedact, SqliteRow.
    let kinds: Vec<&str> = plan
        .items
        .iter()
        .map(|i| match i {
            PurgeItem::Worktree(_) => "worktree",
            PurgeItem::ProviderDir { .. } => "provider",
            PurgeItem::SharedProviderHistory { .. } => "shared-provider",
            PurgeItem::AmqInbox(_) => "amq",
            PurgeItem::LogScopedRedact { .. } => "log",
            PurgeItem::SqliteRow => "sqlite",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["worktree", "provider", "provider", "amq", "log", "sqlite"],
        "purge order must match the contract: worktree → providers → amq → log → sqlite"
    );

    // And `purge` is the module path so dead-code-warning doesn't fire.
    let _module_used: fn(&Path, &str) = |_, _| ();
    let _ = std::convert::identity::<&dyn std::fmt::Debug>(&purge::PurgeOutcome::Done);

    let _keep = h.tmp;
}

#[test]
fn purge_target_resolvable_by_branch_name() {
    let h = PurgeHarness::new();
    // Use the branch name rather than the session id.
    let plan = build_plan(&h.storage, &h.paths, &h.config, "audit02-x").expect("plan by branch");
    assert_eq!(plan.session_id, h.session.id);
    assert_eq!(plan.branch, "audit02-x");
    let _keep = h.tmp;
}

#[test]
fn purge_target_resolves_by_immutable_handle_and_rejects_ambiguous_branch() {
    let h = PurgeHarness::new();
    h.storage.soft_delete_session(&h.session.id).unwrap();
    let by_handle = build_plan(&h.storage, &h.paths, &h.config, h.session.agent_handle())
        .expect("plan by handle");
    assert_eq!(by_handle.session_id, h.session.id);

    let mut sibling = h.session.clone();
    sibling.id = "sid-sibling".to_string();
    sibling.agent_handle = "sid-sibling".to_string();
    sibling.worktree_path = h
        .paths
        .worktrees_root
        .join("sibling")
        .to_string_lossy()
        .into_owned();
    h.storage.upsert_session(&sibling).unwrap();

    let error = build_plan(&h.storage, &h.paths, &h.config, &h.session.branch_name)
        .expect_err("duplicate branch must be ambiguous")
        .to_string();
    assert!(error.contains("more than one session"));
    assert!(error.contains("UUID or agent_handle"));
}

#[test]
fn shared_purge_is_honest_and_retains_identity_without_explicit_consent() {
    let h = PurgeHarness::new();
    let mut shared = h.session.clone();
    shared.shared_workspace = true;
    h.storage.upsert_session(&shared).unwrap();

    let plan = build_plan(&h.storage, &h.paths, &h.config, &shared.id).unwrap();
    assert!(
        !plan
            .items
            .iter()
            .any(|item| matches!(item, PurgeItem::Worktree(_)))
    );
    assert!(
        plan.items
            .iter()
            .all(|item| { !matches!(item, PurgeItem::ProviderDir { .. }) })
    );
    assert!(plan.items.iter().any(|item| matches!(
        item,
        PurgeItem::SharedProviderHistory {
            residual_accepted: false,
            ..
        }
    )));

    let report = execute(&plan, &h.storage, &h.paths, &h.config, false).unwrap();
    assert!(report.had_errors());
    assert!(
        h.worktree().exists(),
        "the registered checkout must survive"
    );
    assert!(
        h.claude_dir.exists(),
        "shared provider history must survive"
    );
    assert!(!h.amq_inbox.exists(), "exact-owned AMQ data may be erased");
    assert!(
        h.storage
            .load_sessions_including_deleted()
            .unwrap()
            .iter()
            .any(|session| session.id == shared.id)
    );
    assert!(report.summary().contains("INCOMPLETE"));
}

#[test]
fn purge_all_retains_shared_provider_history_without_workspace_consent() {
    let h = PurgeHarness::new();
    let transcript = h.claude_dir.join("non-dux-conversation.jsonl");
    fs::write(&transcript, "must survive").unwrap();
    let mut shared = h.session.clone();
    shared.shared_workspace = true;
    h.storage.upsert_session(&shared).unwrap();

    let (plans, failures) = build_plans_for_all(&h.storage, &h.paths, &h.config).unwrap();
    assert!(failures.is_empty());
    assert_eq!(plans.len(), 1);
    assert!(plans[0].items.iter().any(|item| matches!(
        item,
        PurgeItem::SharedProviderHistory {
            residual_accepted: false,
            ..
        }
    )));
    assert!(
        !plans[0]
            .items
            .iter()
            .any(|item| matches!(item, PurgeItem::ProviderDir { .. }))
    );

    let report = execute(&plans[0], &h.storage, &h.paths, &h.config, false).unwrap();
    assert!(report.had_errors());
    assert!(report.summary().contains("INCOMPLETE"));
    assert_eq!(fs::read_to_string(transcript).unwrap(), "must survive");
    assert!(!h.amq_inbox.exists(), "exact-owned AMQ data may be erased");
    assert_eq!(
        h.storage.load_sessions_including_deleted().unwrap().len(),
        1,
        "the recovery row must remain"
    );
}

#[test]
fn shared_purge_accept_residual_frees_exact_amq_and_deletes_row_only() {
    let h = PurgeHarness::new();
    let mut shared = h.session.clone();
    shared.shared_workspace = true;
    h.storage.upsert_session(&shared).unwrap();

    let plan = build_plans_for_target(
        &h.storage,
        &h.paths,
        &h.config,
        &shared.id,
        SharedPurgeMode::AcceptResidualData,
    )
    .unwrap()
    .remove(0);
    let report = execute(&plan, &h.storage, &h.paths, &h.config, false).unwrap();

    assert!(!report.had_errors(), "{}", report.summary());
    assert!(h.worktree().exists());
    assert!(h.claude_dir.exists());
    assert!(!h.amq_inbox.exists());
    assert!(
        h.storage
            .load_sessions_including_deleted()
            .unwrap()
            .is_empty()
    );
    assert!(report.summary().contains("RESIDUAL ACCEPTED"));
}

#[test]
fn workspace_wide_shared_purge_includes_every_sibling_and_provider_history() {
    let h = PurgeHarness::new();
    let mut shared = h.session.clone();
    shared.shared_workspace = true;
    h.storage.upsert_session(&shared).unwrap();

    let mut sibling = shared.clone();
    sibling.id = "sid-sibling".to_string();
    sibling.agent_handle = "sid-sibling".to_string();
    h.storage.upsert_session(&sibling).unwrap();
    write_owned_amq_marker(&h, &sibling, &["sid-target", "sid-sibling"]);

    let plans = build_plans_for_target(
        &h.storage,
        &h.paths,
        &h.config,
        &shared.id,
        SharedPurgeMode::WorkspaceWide,
    )
    .unwrap();
    assert_eq!(plans.len(), 2);
    assert!(plans.iter().all(|plan| {
        !plan
            .items
            .iter()
            .any(|item| matches!(item, PurgeItem::Worktree(_)))
            && plan
                .items
                .iter()
                .any(|item| matches!(item, PurgeItem::ProviderDir { .. }))
    }));

    for plan in &plans {
        let report = execute(plan, &h.storage, &h.paths, &h.config, false).unwrap();
        assert!(!report.had_errors(), "{}", report.summary());
    }
    assert!(h.worktree().exists());
    assert!(!h.claude_dir.exists());
    assert!(
        h.storage
            .load_sessions_including_deleted()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn purge_never_frees_a_foreign_amq_inbox() {
    let h = PurgeHarness::new();
    fs::write(
        h.amq_inbox.join(".dux-amq-source"),
        r#"{"store_id":"foreign-store","session_id":"sid-target"}"#,
    )
    .unwrap();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).unwrap();
    let report = execute(&plan, &h.storage, &h.paths, &h.config, false).unwrap();

    assert!(report.had_errors());
    assert!(h.amq_inbox.exists());
    assert_eq!(
        h.storage.load_sessions_including_deleted().unwrap().len(),
        1
    );
}

#[test]
fn purge_worktree_step_rejects_registered_project_overlap_in_both_directions() {
    let h = PurgeHarness::new();
    for project in [h.worktree().join("nested-project"), h.paths.root.clone()] {
        let mut config = h.config.clone();
        config.registered_project_paths = vec![project];
        assert!(build_plan(&h.storage, &h.paths, &config, &h.session.id).is_err());
        assert!(h.worktree().exists());
    }

    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).unwrap();
    let mut changed_inventory = h.config.clone();
    changed_inventory.registered_project_paths = vec![h.worktree().join("late-project")];
    let report = execute(&plan, &h.storage, &h.paths, &changed_inventory, false).unwrap();
    assert!(report.had_errors());
    assert!(h.worktree().exists());
    assert_eq!(
        h.storage.load_sessions_including_deleted().unwrap().len(),
        1
    );
}

#[test]
fn purge_retains_row_after_each_recursive_category_failure_and_retries() {
    for category in ["worktree", "provider", "amq"] {
        let h = PurgeHarness::new();
        let fault = h.tmp.path().join(format!("{category}-fault"));
        fs::write(&fault, b"not a directory").unwrap();
        let item = match category {
            "worktree" => PurgeItem::Worktree(fault),
            "provider" => PurgeItem::ProviderDir {
                provider: "claude",
                path: fault,
            },
            "amq" => PurgeItem::AmqInbox(fault),
            _ => unreachable!(),
        };

        assert_failure_retains_row_and_retry_succeeds(&h, item, &h.paths, category);
    }
}

#[test]
fn purge_retains_row_after_log_failure_and_retries() {
    let h = PurgeHarness::new();
    let log_root_fault = h.tmp.path().join("log-root-fault");
    fs::write(&log_root_fault, b"not a directory").unwrap();
    let mut faulty_paths = h.paths.clone();
    faulty_paths.root = log_root_fault.clone();

    assert_failure_retains_row_and_retry_succeeds(
        &h,
        PurgeItem::LogScopedRedact {
            since: h.session.created_at,
            path: log_root_fault.join("dux.log"),
        },
        &faulty_paths,
        "log",
    );
}

#[test]
fn purge_rejects_root_parent_and_symlink_worktree_targets() {
    let h = PurgeHarness::new();

    for unsafe_path in [
        PathBuf::from("/"),
        h.paths.worktrees_root.clone(),
        h.paths.worktrees_root.join("safe/../../outside"),
    ] {
        let mut session = h.session.clone();
        session.worktree_path = unsafe_path.to_string_lossy().into_owned();
        assert!(
            plan_for_session(&session, &h.paths, &h.config).is_err(),
            "accepted unsafe worktree {}",
            unsafe_path.display()
        );
    }

    let outside = h.tmp.path().join("outside-worktrees");
    fs::create_dir(&outside).unwrap();
    let escape = h.paths.worktrees_root.join("escape");
    symlink(&outside, &escape).unwrap();
    let mut session = h.session.clone();
    session.worktree_path = escape.to_string_lossy().into_owned();
    assert!(plan_for_session(&session, &h.paths, &h.config).is_err());
}

#[test]
fn purge_amq_target_uses_handle_even_when_branch_is_pathlike() {
    let h = PurgeHarness::new();
    for branch in ["/tmp/audit03-outside", "../../audit03-outside", "."] {
        let mut session = h.session.clone();
        session.branch_name = branch.to_string();
        let plan = plan_for_session(&session, &h.paths, &h.config).expect("handle-keyed plan");
        assert!(
            plan.items.iter().any(
                |item| matches!(item, PurgeItem::AmqInbox(path) if path.ends_with("sid-target"))
            )
        );
    }
}

#[test]
fn purge_rejects_provider_and_amq_symlink_escapes() {
    let provider = PurgeHarness::new();
    let outside_provider = provider.tmp.path().join("outside-provider");
    fs::create_dir(&outside_provider).unwrap();
    fs::remove_dir_all(&provider.claude_dir).unwrap();
    symlink(&outside_provider, &provider.claude_dir).unwrap();
    assert!(plan_for_session(&provider.session, &provider.paths, &provider.config).is_err());

    let amq = PurgeHarness::new();
    let outside_amq = amq.tmp.path().join("outside-amq");
    fs::create_dir(&outside_amq).unwrap();
    fs::remove_dir_all(&amq.amq_inbox).unwrap();
    symlink(&outside_amq, &amq.amq_inbox).unwrap();
    assert!(plan_for_session(&amq.session, &amq.paths, &amq.config).is_err());
}

#[test]
fn purge_allows_missing_descendants_and_reports_validated_paths() {
    let h = PurgeHarness::new();
    let mut session = h.session.clone();
    let missing_worktree = h.paths.worktrees_root.join("missing/session");
    session.worktree_path = missing_worktree.to_string_lossy().into_owned();
    session.branch_name = "missing/session".to_string();

    let plan = plan_for_session(&session, &h.paths, &h.config).expect("missing descendants");
    let planned_worktree = match &plan.items[0] {
        PurgeItem::Worktree(path) => path,
        other => panic!("unexpected first item: {other:?}"),
    };
    let expected = h
        .paths
        .worktrees_root
        .canonicalize()
        .unwrap()
        .join("missing/session");
    assert_eq!(planned_worktree, &expected);
    assert!(
        plan.items[0]
            .describe()
            .contains(&expected.display().to_string())
    );
}

#[test]
fn purge_all_retains_rows_whose_full_inventory_cannot_be_planned() {
    let h = PurgeHarness::new();
    let mut relative_worktree = h.session.clone();
    relative_worktree.id = "sid-relative\u{1b}]8;;bad".to_string();
    relative_worktree.agent_handle = "sid-relative-bad".to_string();
    relative_worktree.branch_name = "relative-row".to_string();
    relative_worktree.worktree_path = "relative/worktree".to_string();
    h.storage
        .upsert_session(&relative_worktree)
        .expect("insert relative worktree row");

    let mut absolute_branch = h.session.clone();
    absolute_branch.id = "sid-absolute-branch".to_string();
    absolute_branch.agent_handle = "sid-absolute-branch".to_string();
    absolute_branch.branch_name = "/tmp/bad\u{1b}]0;branch".to_string();
    absolute_branch.worktree_path = h
        .paths
        .worktrees_root
        .join("missing-absolute-branch-row")
        .to_string_lossy()
        .into_owned();
    h.storage
        .upsert_session(&absolute_branch)
        .expect("insert absolute branch row");

    let (plans, failures) =
        build_plans_for_all(&h.storage, &h.paths, &h.config).expect("bulk plans");

    assert_eq!(plans.len(), 2);
    assert_eq!(failures.len(), 1);
    assert!(failures.iter().all(|failure| !failure.contains('\u{1b}')));
    assert!(
        failures
            .iter()
            .all(|failure| failure.contains("was retained"))
    );
    assert!(
        plans
            .iter()
            .any(|plan| plan.session_id == h.session.id && plan.items.len() > 1),
        "the valid session must retain its full purge plan"
    );

    for plan in &plans {
        let report =
            execute(plan, &h.storage, &h.paths, &h.config, false).expect("execute bulk plan");
        assert!(!report.had_errors(), "{}", report.summary());
    }
    let remaining = h
        .storage
        .load_sessions_including_deleted()
        .expect("load rows");
    assert_eq!(remaining.len(), 1);
    assert!(remaining.iter().all(|session| session.id != h.session.id));
}
