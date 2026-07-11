//! GDPR hard-purge: cascade-delete every byte associated with a session.
//!
//! This module implements audit02 Phase 10 (P0-J / GDPR Art 17
//! right-to-erasure). Today's `dux config reset --all` removes worktrees
//! and the sqlite database holistically but cannot target a single
//! session, and never touches the per-session provider chat history under
//! `<provider_root>/projects/<encoded>/` or the AMQ inbox under
//! `<amq_root>/agents/<branch>/`. Without those paths a real "delete this
//! customer's data" request is impossible.
//!
//! ## What gets purged
//!
//! Given a session identified by uuid OR branch name, this module
//! removes:
//!
//! 1. The session's worktree directory (refusing to touch anything
//!    outside the configured worktrees root — defence in depth against
//!    a malformed db row pointing at `/`).
//! 2. The Claude / Codex / Gemini chat-history dirs at
//!    `<provider_root>/<provider>/projects/<encoded>` where
//!    `<encoded>` is computed by `crate::purge_encoding`.
//! 3. The session's AMQ inbox at `<amq_root>/agents/<branch>` (we only
//!    delete the branch-named directory, never the parent — peers'
//!    inboxes must remain untouched).
//! 4. Log records tagged with this `session_id`. These are *redacted*
//!    rather than deleted: every JSON Lines record in `dux.log*` is
//!    streamed through, and any record whose `fields.session_id`
//!    matches the target has its `fields` object replaced with
//!    `{"redacted": true}`. The audit trail (this session was purged
//!    on this date) survives; the content does not.
//! 5. The `agent_sessions` row in `sessions.sqlite3`.
//!
//! ## Order of operations
//!
//! Worktree → providers → AMQ → log redact → sqlite. The sqlite row is
//! deleted **last** so that a crash mid-purge leaves a recoverable
//! record: the operator can re-run `dux session purge --hard <id>` and
//! it will resume cleanly. If we deleted the sqlite row first and then
//! crashed during worktree removal, we'd have orphaned bytes on disk
//! with no record of which session they belonged to.
//!
//! ## Dry-run discipline
//!
//! Every step honors `dry_run`. The integration tests assert this
//! explicitly per category, because a bug where one step ignores the
//! flag would cause silent data loss in test runs.
//!
//! ## Cross-fs safety
//!
//! Provider dirs and the worktree often live on different filesystems
//! (persistent disk vs boot disk). We use `std::fs::remove_dir_all`
//! which handles cross-fs deletion natively; we never `rename`-then-
//! delete (which would fail across mount points).

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::config::{DuxPaths, LoggingConfig};
use crate::model::AgentSession;
use crate::purge_encoding;
use crate::sanitize;
use crate::storage::SessionStore;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration the purge cascade needs that is not derivable from
/// `DuxPaths` alone. Today this is purely the per-provider data-dir
/// roots; future fields (e.g. extra log directories, custom AMQ root)
/// can grow here without churning the public API.
#[derive(Clone, Debug)]
pub struct PurgeConfig {
    /// Per-provider chat-history roots. Each entry maps a logical
    /// provider name (`"claude"`, `"codex"`, `"gemini"`) to the
    /// directory whose `projects/<encoded>` subtree should be purged.
    /// On the dux-amq VM these default to `/data/state/<provider>`.
    pub provider_data_dirs: Vec<(String, PathBuf)>,
    /// Root of the AMQ file-bus. The session's branch-named inbox
    /// lives at `<amq_root>/agents/<branch>`. Defaults to
    /// `/data/state/amq`.
    pub amq_root: PathBuf,
    /// Resolved live log prefix (absolute or relative-to-Dux root exactly as
    /// the logger resolves it). Rotated siblings share this prefix.
    pub log_path: PathBuf,
}

impl PurgeConfig {
    /// Build a sensible default `PurgeConfig` for the production layout
    /// described in `dux-amq/README.md`. Tests override this with a
    /// scratch directory.
    pub fn default_layout(paths: &DuxPaths, logging: &LoggingConfig) -> Self {
        let state = std::env::var_os("STATE_ROOT")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| paths.root.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| paths.root.clone());
        Self {
            provider_data_dirs: vec![
                ("claude".to_string(), state.join("claude")),
                ("codex".to_string(), state.join("codex")),
                ("gemini".to_string(), state.join("gemini")),
            ],
            amq_root: state.join("amq"),
            log_path: crate::logger::resolve_log_path(logging, paths),
        }
    }
}

/// One unit of work in a `PurgePlan`. Each variant maps to a single
/// `execute_*` arm so dry-run reporting and real execution share a
/// single dispatch table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeItem {
    /// The `agent_sessions` row keyed by `session_id`.
    SqliteRow,
    /// Recursive delete of the worktree directory.
    Worktree(PathBuf),
    /// Recursive delete of `<provider_root>/projects/<encoded>`.
    ProviderDir {
        provider: &'static str,
        path: PathBuf,
    },
    /// Recursive delete of `<amq_root>/agents/<branch>`. The parent
    /// `agents/` dir is never removed.
    AmqInbox(PathBuf),
    /// Streaming JSON-line rewrite of every `dux.log*` file under
    /// `paths.root`, redacting records whose `fields.session_id`
    /// matches the target.
    LogScopedRedact {
        /// Earliest timestamp to consider. Currently informational
        /// (we rewrite every record regardless), but recorded so the
        /// audit trail shows the cut-over point if a future revision
        /// wants to skip pre-baseline lines.
        since: DateTime<Utc>,
        path: PathBuf,
    },
}

impl PurgeItem {
    /// Operator-facing one-line description. Used by the confirmation
    /// prompt and the dry-run report.
    pub fn describe(&self) -> String {
        match self {
            Self::SqliteRow => "sqlite row in sessions.sqlite3".to_string(),
            Self::Worktree(p) => format!("worktree {}", safe_path(p)),
            Self::ProviderDir { provider, path } => {
                format!("{provider} chat history {}", safe_path(path))
            }
            Self::AmqInbox(p) => format!("amq inbox {}", safe_path(p)),
            Self::LogScopedRedact { since, path } => {
                format!(
                    "redact {} records since {}",
                    safe_path(path),
                    since.to_rfc3339()
                )
            }
        }
    }
}

/// A complete plan for purging exactly one session. Built by
/// [`build_plan`] and consumed by [`execute`]. The plan is also what
/// the confirmation prompt and dry-run output enumerate.
#[derive(Clone, Debug)]
pub struct PurgePlan {
    pub session_id: String,
    pub branch: String,
    pub items: Vec<PurgeItem>,
}

/// Per-item outcome reported by [`execute`]. `Skipped` means the item
/// was a no-op (e.g. directory already absent); `DryRun` means
/// `dry_run = true` was honored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    Done,
    Skipped(String),
    DryRun,
    Error(String),
}

/// The audit-trail record returned by [`execute`]. One entry per
/// `PurgeItem`, in execution order.
#[derive(Clone, Debug)]
pub struct PurgeReport {
    pub session_id: String,
    pub branch: String,
    pub entries: Vec<(PurgeItem, PurgeOutcome)>,
    pub dry_run: bool,
}

impl PurgeReport {
    /// Operator-facing summary suitable for printing to stderr after
    /// a successful purge.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let prefix = if self.dry_run { "DRY-RUN " } else { "" };
        s.push_str(&format!(
            "{prefix}purge report for session {} (branch {}):\n",
            sanitize::for_terminal(&self.session_id),
            sanitize::for_terminal(&self.branch)
        ));
        for (item, outcome) in &self.entries {
            let tag = match outcome {
                PurgeOutcome::Done => "ok",
                PurgeOutcome::Skipped(_) => "skip",
                PurgeOutcome::DryRun => "dry-run",
                PurgeOutcome::Error(_) => "ERROR",
            };
            s.push_str(&format!("  [{tag}] {}\n", item.describe()));
            if let PurgeOutcome::Skipped(why) | PurgeOutcome::Error(why) = outcome {
                s.push_str(&format!("        ({})\n", sanitize::for_terminal(why)));
            }
        }
        s
    }

    /// Did any item end in `Error`? Used by the CLI to set a non-zero
    /// exit code even when the cascade nominally completes.
    pub fn had_errors(&self) -> bool {
        self.entries
            .iter()
            .any(|(_, o)| matches!(o, PurgeOutcome::Error(_)))
    }
}

// ---------------------------------------------------------------------------
// Plan construction
// ---------------------------------------------------------------------------

/// Resolve `target` to an `AgentSession` by id or branch name and build
/// the cascade plan. Errors if no session matches.
pub fn build_plan(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
    target: &str,
) -> Result<PurgePlan> {
    let sessions = storage
        .load_sessions()
        .context("failed to load sessions for purge planning")?;
    let session = sessions
        .iter()
        .find(|s| s.id == target || s.branch_name == target)
        .ok_or_else(|| {
            anyhow!(
                "no session found matching id or branch {:?}",
                sanitize::for_terminal(target)
            )
        })?;
    plan_for_session(session, paths, config)
}

/// Lower-level helper used by `build_plan` and by the `purge_all`
/// fan-out. Caller has already resolved the session.
pub fn plan_for_session(
    session: &AgentSession,
    paths: &DuxPaths,
    config: &PurgeConfig,
) -> Result<PurgePlan> {
    let mut items = Vec::new();

    // 1. Worktree
    let worktree = validate_delete_target(
        &paths.worktrees_root,
        Path::new(&session.worktree_path),
        "worktree",
    )?;
    items.push(PurgeItem::Worktree(worktree));

    // 2. Provider dirs — encoded from the worktree's absolute path.
    //    A relative or non-UTF-8 worktree path yields an error: we
    //    refuse to silently mis-encode and orphan a chat-history dir.
    let encoded = purge_encoding::encode_str(&session.worktree_path).with_context(|| {
        format!(
            "failed to encode worktree path {:?} for provider dir lookup",
            sanitize::for_terminal(&session.worktree_path),
        )
    })?;
    for (provider_name, root) in &config.provider_data_dirs {
        let provider_static: &'static str = static_provider_name(provider_name);
        let provider_projects = root.join("projects");
        let path = validate_delete_target(
            &provider_projects,
            &provider_projects.join(&encoded),
            "provider history",
        )?;
        items.push(PurgeItem::ProviderDir {
            provider: provider_static,
            path,
        });
    }

    // 3. AMQ inbox. Reject malformed persisted branch paths even when the
    // runtime identity resolves from the worktree basename instead.
    let branch_path = Path::new(&session.branch_name);
    if session.branch_name.is_empty()
        || branch_path.is_absolute()
        || branch_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::CurDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        bail!(
            "unsafe AMQ branch path: {}",
            sanitize::for_terminal(&session.branch_name)
        );
    }

    // Use the exact runtime identity priority (sanitised worktree basename,
    // then branch, then id), not the raw DB branch.
    let handle = crate::peer::amq_handle_for_session(session);
    if !handle.is_empty() {
        let agents_root = config.amq_root.join("agents");
        let inbox = validate_delete_target(&agents_root, &agents_root.join(&handle), "AMQ inbox")?;
        items.push(PurgeItem::AmqInbox(inbox));
    }

    // 4. Log redact (Phase 09 dep). `since` is informational — the
    //    streaming rewriter visits every line in every rotated log
    //    regardless. Keeping it on the item lets future revisions
    //    short-circuit older files cheaply.
    items.push(PurgeItem::LogScopedRedact {
        since: session.created_at,
        path: config.log_path.clone(),
    });

    // 5. Sqlite row LAST.
    items.push(PurgeItem::SqliteRow);

    Ok(PurgePlan {
        session_id: session.id.clone(),
        branch: session.branch_name.clone(),
        items,
    })
}

/// Resolve an existing path through symlinks, or resolve its deepest existing
/// ancestor and append the missing suffix. This lets purge validate idempotent
/// (already-missing) targets without weakening containment.
fn resolve_for_containment(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("purge path must be absolute: {}", safe_path(path));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        bail!("purge path contains parent traversal: {}", safe_path(path));
    }

    let mut ancestor = path;
    let mut missing = Vec::<OsString>::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor
                    .file_name()
                    .ok_or_else(|| anyhow!("could not resolve purge path {}", safe_path(path)))?;
                missing.push(name.to_os_string());
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| anyhow!("could not resolve purge path {}", safe_path(path)))?;
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to inspect purge path {}", safe_path(path)));
            }
        }
    }

    let mut resolved = ancestor
        .canonicalize()
        .with_context(|| format!("failed to resolve purge path {}", safe_path(path)))?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn validate_delete_target(root: &Path, target: &Path, category: &str) -> Result<PathBuf> {
    let root = resolve_for_containment(root)
        .with_context(|| format!("failed to resolve {category} root {}", safe_path(root)))?;
    let target = resolve_for_containment(target)
        .with_context(|| format!("failed to resolve {category} target {}", safe_path(target)))?;
    if target == root {
        bail!(
            "refusing to purge {category} root itself: {}",
            safe_path(&target)
        );
    }
    if !target.starts_with(&root) {
        bail!(
            "refusing {category} target outside {}: {}",
            safe_path(&root),
            safe_path(&target)
        );
    }
    Ok(target)
}

fn safe_path(path: &Path) -> String {
    sanitize::for_terminal(&path.display().to_string())
}

/// Map the dynamic provider name (read from config) to a `&'static str`
/// suitable for embedding in `PurgeItem::ProviderDir`. We restrict to
/// the three providers we know how to encode for; unknown names get
/// the catch-all `"other"` label so report output stays readable
/// without leaking arbitrary user-supplied strings into the type.
fn static_provider_name(name: &str) -> &'static str {
    match name {
        "claude" => "claude",
        "codex" => "codex",
        "gemini" => "gemini",
        "opencode" => "opencode",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute the cascade. Returns a [`PurgeReport`] with per-item
/// outcomes. `dry_run = true` short-circuits every destructive step
/// to a `DryRun` outcome; nothing is mutated on disk.
pub fn execute(
    plan: &PurgePlan,
    storage: &SessionStore,
    _paths: &DuxPaths,
    dry_run: bool,
) -> Result<PurgeReport> {
    let safe_session_id = sanitize::for_terminal(&plan.session_id);
    let safe_branch = sanitize::for_terminal(&plan.branch);
    tracing::info!(
        target: "dux::purge",
        session_id = %safe_session_id,
        branch = %safe_branch,
        items = plan.items.len(),
        dry_run = dry_run,
        "purge cascade starting",
    );

    let mut entries: Vec<(PurgeItem, PurgeOutcome)> = Vec::with_capacity(plan.items.len());
    for item in &plan.items {
        let prior_step_failed = entries
            .iter()
            .any(|(_, outcome)| matches!(outcome, PurgeOutcome::Error(_)));
        let outcome = if matches!(item, PurgeItem::SqliteRow) && prior_step_failed {
            PurgeOutcome::Skipped(
                "prior purge step failed; session row retained for retry".to_string(),
            )
        } else {
            execute_item(item, storage, &plan.session_id, dry_run)
        };
        match &outcome {
            PurgeOutcome::Error(why) => {
                tracing::error!(
                    target: "dux::purge",
                    session_id = %safe_session_id,
                    item = %item.describe(),
                    err = %sanitize::for_terminal(why),
                    "purge step failed",
                );
            }
            PurgeOutcome::Skipped(why) => {
                tracing::info!(
                    target: "dux::purge",
                    session_id = %safe_session_id,
                    item = %item.describe(),
                    reason = %sanitize::for_terminal(why),
                    "purge step skipped",
                );
            }
            PurgeOutcome::DryRun | PurgeOutcome::Done => {
                tracing::info!(
                    target: "dux::purge",
                    session_id = %safe_session_id,
                    item = %item.describe(),
                    "purge step complete",
                );
            }
        }
        entries.push((item.clone(), outcome));
    }

    Ok(PurgeReport {
        session_id: plan.session_id.clone(),
        branch: plan.branch.clone(),
        entries,
        dry_run,
    })
}

fn execute_item(
    item: &PurgeItem,
    storage: &SessionStore,
    session_id: &str,
    dry_run: bool,
) -> PurgeOutcome {
    match item {
        PurgeItem::Worktree(p) => execute_remove_dir(p, dry_run),
        PurgeItem::ProviderDir { path, .. } => execute_remove_dir(path, dry_run),
        PurgeItem::AmqInbox(p) => execute_remove_dir(p, dry_run),
        PurgeItem::LogScopedRedact { path, .. } => execute_redact_logs(path, session_id, dry_run),
        PurgeItem::SqliteRow => execute_delete_row(storage, session_id, dry_run),
    }
}

fn execute_remove_dir(path: &Path, dry_run: bool) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PurgeOutcome::Skipped(format!("{} does not exist", path.display()));
        }
        Err(err) => {
            return PurgeOutcome::Error(format!("inspect {}: {err}", path.display()));
        }
    }
    match fs::remove_dir_all(path) {
        Ok(()) => PurgeOutcome::Done,
        Err(e) => PurgeOutcome::Error(format!("remove_dir_all({}): {e}", path.display())),
    }
}

fn execute_delete_row(storage: &SessionStore, session_id: &str, dry_run: bool) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    match storage.delete_session(session_id) {
        Ok(()) => PurgeOutcome::Done,
        Err(e) => PurgeOutcome::Error(format!("delete_session({session_id}): {e}")),
    }
}

// ---------------------------------------------------------------------------
// Log redaction (Phase 09 dep)
// ---------------------------------------------------------------------------

/// Stream-rewrite every `dux.log*` file under `paths.root`, replacing the
/// `fields` object of any JSON Lines record whose `fields.session_id`
/// matches `session_id`. Each rewritten file is staged in a sibling
/// `.purge.tmp` file and atomically renamed into place when the rewrite
/// completes; a crash mid-rewrite leaves the original file intact.
///
/// Returns `Done` even when no records match — the audit trail is the
/// fact that the rewrite ran, not the count of redacted lines.
fn execute_redact_logs(log_path: &Path, session_id: &str, dry_run: bool) -> PurgeOutcome {
    let Some(log_root) = log_path.parent() else {
        return PurgeOutcome::Error(format!("log path has no parent: {}", safe_path(log_path)));
    };
    let log_root = match fs::metadata(log_root) {
        Ok(_) => log_root.to_path_buf(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PurgeOutcome::Skipped(format!(
                "log root {} does not exist",
                safe_path(log_root)
            ));
        }
        Err(err) => {
            return PurgeOutcome::Error(format!("inspect log root {}: {err}", safe_path(log_root)));
        }
    };

    let log_files = match collect_log_files(log_path) {
        Ok(v) => v,
        Err(e) => return PurgeOutcome::Error(format!("scan {log_root:?}: {e}")),
    };
    if log_files.is_empty() {
        return PurgeOutcome::Skipped(format!(
            "no {} log files under {}",
            log_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("configured"),
            safe_path(&log_root)
        ));
    }

    if dry_run {
        return PurgeOutcome::DryRun;
    }

    let mut total_redacted: usize = 0;
    for file in &log_files {
        match redact_one_log_file(file, session_id) {
            Ok(n) => total_redacted += n,
            Err(e) => {
                return PurgeOutcome::Error(format!("redact {} failed: {e}", file.display()));
            }
        }
    }
    let _ = total_redacted; // kept for future telemetry; tracing already covered.
    PurgeOutcome::Done
}

/// Collect every file under `root` whose name starts with `dux.log`.
/// This matches the live `dux.log` plus all rotated `dux.log.YYYY-MM-DD`
/// children that `tracing-appender` produces.
fn collect_log_files(log_path: &Path) -> Result<Vec<PathBuf>> {
    let root = log_path
        .parent()
        .ok_or_else(|| anyhow!("log path has no parent: {}", safe_path(log_path)))?;
    let prefix = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("log filename is not UTF-8: {}", safe_path(log_path)))?;
    let mut out = Vec::new();
    walk_collect_log_files(root, prefix, &mut out)?;
    Ok(out)
}

fn walk_collect_log_files(dir: &Path, prefix: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("read_dir {}", safe_path(dir)))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read_dir entry under {}", safe_path(dir)))?;
        let ty = entry
            .file_type()
            .with_context(|| format!("file_type {}", entry.path().display()))?;
        if ty.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name == prefix || name.starts_with(&format!("{prefix}.")))
        {
            out.push(entry.path());
        }
        // Intentional: do not recurse into subdirectories. The
        // log-rotation appender writes flat files only; recursing
        // would risk touching unrelated `dux.log*` files that a user
        // happens to keep under a worktree (which we already
        // delete via PurgeItem::Worktree).
    }
    Ok(())
}

/// Streaming line-rewrite of one log file. Returns the count of
/// redacted lines.
fn redact_one_log_file(path: &Path, session_id: &str) -> Result<usize> {
    let src = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(src);

    let tmp_path = path.with_extension({
        // Preserve the rotation suffix by appending `.purge.tmp`
        // rather than replacing the existing extension.
        let mut ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if !ext.is_empty() {
            ext.push('.');
        }
        ext.push_str("purge.tmp");
        ext
    });

    let mut tmp =
        fs::File::create(&tmp_path).with_context(|| format!("create {}", tmp_path.display()))?;

    let mut redacted = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .with_context(|| format!("read_line {}", path.display()))?;
        if n == 0 {
            break;
        }
        // Preserve trailing newline behaviour: strip it for parsing,
        // re-add when writing.
        let had_newline = line.ends_with('\n');
        let payload: &str = if had_newline {
            &line[..line.len() - 1]
        } else {
            &line[..]
        };

        let rewritten = match maybe_redact_json_line(payload, session_id) {
            Some(new_line) => {
                redacted += 1;
                new_line
            }
            None => payload.to_string(),
        };

        tmp.write_all(rewritten.as_bytes())
            .with_context(|| format!("write {}", tmp_path.display()))?;
        if had_newline {
            tmp.write_all(b"\n")
                .with_context(|| format!("write newline to {}", tmp_path.display()))?;
        }
    }
    tmp.sync_all()
        .with_context(|| format!("sync {}", tmp_path.display()))?;
    drop(tmp);

    // Atomic rename. On Unix this is a rename(2) and is durable across
    // a crash provided the parent dir's metadata reaches disk; we
    // accept the standard ext4 / apfs guarantees here.
    fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;
    Ok(redacted)
}

/// If `line` is a JSON object whose `fields.session_id` equals `target`,
/// return a rewritten version where `fields` is replaced with
/// `{"redacted": true}`. Otherwise return `None` and the caller
/// preserves the original line untouched.
fn maybe_redact_json_line(line: &str, target: &str) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    let mut parsed: Value = serde_json::from_str(line).ok()?;
    let obj = parsed.as_object_mut()?;
    let matches = obj
        .get("fields")
        .and_then(|f| f.get("session_id"))
        .and_then(|sid| sid.as_str())
        .map(|sid| sid == target)
        .unwrap_or(false);
    if !matches {
        return None;
    }
    let mut redacted = Map::new();
    redacted.insert("redacted".to_string(), Value::Bool(true));
    obj.insert("fields".to_string(), Value::Object(redacted));
    serde_json::to_string(&parsed).ok()
}

// ---------------------------------------------------------------------------
// Confirmation prompt
// ---------------------------------------------------------------------------

/// Read a confirmation token from `reader` and check that it equals the
/// literal string `"PURGE <branch>"`. Using "PURGE <branch>" as the
/// magic phrase prevents accidental yes-mashing — the operator must
/// type the branch name explicitly.
pub fn confirm_with_reader<R: BufRead>(plan: &PurgePlan, reader: &mut R) -> Result<bool> {
    let mut input = String::new();
    reader
        .read_line(&mut input)
        .context("failed to read confirmation from stdin")?;
    let expected = format!("PURGE {}", plan.branch);
    Ok(input.trim() == expected)
}

/// Convenience wrapper that prints the plan to `stderr`, asks for the
/// confirmation phrase on `stderr`, and reads from `stdin`. Test code
/// should call `confirm_with_reader` directly with a fixture reader.
pub fn confirm_interactive(plan: &PurgePlan) -> Result<bool> {
    eprintln!(
        "Will permanently delete the following for session {} (branch {}):",
        plan.session_id, plan.branch
    );
    for item in &plan.items {
        eprintln!("  - {}", item.describe());
    }
    eprint!("Type 'PURGE {}' to confirm: ", plan.branch);
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    confirm_with_reader(plan, &mut handle)
}

// ---------------------------------------------------------------------------
// AMQ peer notification
// ---------------------------------------------------------------------------

/// Send a final `purge` notification to the AMQ bus before deleting the
/// branch's inbox so peers see "this agent is gone" rather than silently
/// stop receiving acks. Failure is non-fatal: we log it and continue —
/// purge correctness must not depend on a working AMQ command.
///
/// On systems without `amq` on PATH this is a no-op. The function is
/// pulled out of `execute` so tests can short-circuit it.
pub fn notify_amq_peers_of_purge(branch: &str) {
    let safe_branch = sanitize::for_terminal(branch);
    let body =
        format!("agent {safe_branch} purged via dux session purge --hard; inbox will be removed");
    let result = std::process::Command::new("amq")
        .args(["send", branch, "--label", "purge", &body])
        .output();
    match result {
        Ok(out) if out.status.success() => {
            tracing::info!(
                target: "dux::purge",
                branch = %safe_branch,
                "amq peer notification sent",
            );
        }
        Ok(out) => {
            tracing::warn!(
                target: "dux::purge",
                branch = %safe_branch,
                stderr = %sanitize::utf8_lossy(&out.stderr),
                "amq peer notification failed (non-fatal)",
            );
        }
        Err(e) => {
            tracing::debug!(
                target: "dux::purge",
                branch = %safe_branch,
                err = %e,
                "amq command not available; skipping peer notification",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Bulk purge-all
// ---------------------------------------------------------------------------

/// Build plans for every session in storage. Used by `dux session
/// purge-all`. Returns plans in load order (most-recently-updated first) and
/// one operator-facing warning for each malformed row reduced to a row-only
/// purge.
pub fn build_plans_for_all(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
) -> Result<(Vec<PurgePlan>, Vec<String>)> {
    let sessions = storage
        .load_sessions()
        .context("failed to load sessions for bulk purge planning")?;
    if sessions.is_empty() {
        bail!("no sessions found; nothing to purge");
    }
    let mut plans = Vec::with_capacity(sessions.len());
    let mut failures = Vec::new();
    for session in &sessions {
        match plan_for_session(session, paths, config) {
            Ok(plan) => plans.push(plan),
            Err(err) => {
                failures.push(format!(
                    "session {:?} (branch {:?}) has unsafe purge targets: {}; using row-only purge",
                    sanitize::for_terminal(&session.id),
                    sanitize::for_terminal(&session.branch_name),
                    sanitize::for_terminal(&format!("{err:#}")),
                ));
                plans.push(PurgePlan {
                    session_id: session.id.clone(),
                    branch: session.branch_name.clone(),
                    items: vec![PurgeItem::SqliteRow],
                });
            }
        }
    }
    Ok((plans, failures))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProviderKind, SessionState};

    fn fixture_paths(root: &Path) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.to_path_buf(),
        }
    }

    fn fixture_session(id: &str, branch: &str, worktree: &Path) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            project_id: "proj".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            state: SessionState::Created { created_at: now },
            settings: crate::model::SessionSettings::default(),
            created_at: now,
            updated_at: now,
        }
    }

    fn fixture_config(amq_root: PathBuf, providers: &[(&str, PathBuf)]) -> PurgeConfig {
        PurgeConfig {
            provider_data_dirs: providers
                .iter()
                .map(|(n, p)| ((*n).to_string(), p.clone()))
                .collect(),
            amq_root,
            log_path: PathBuf::from("/tmp/dux.log"),
        }
    }

    #[test]
    fn containment_errors_escape_terminal_controls_in_every_path_bail() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root\u{1b}");
        let outside = temp.path().join("outside\u{1b}");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();

        let errors = [
            resolve_for_containment(Path::new("relative\u{1b}"))
                .expect_err("relative path must fail"),
            resolve_for_containment(&temp.path().join("safe/../parent\u{1b}"))
                .expect_err("parent traversal must fail"),
            validate_delete_target(&root, &root, "test").expect_err("root target must fail"),
            validate_delete_target(&root, &outside, "test").expect_err("outside target must fail"),
        ];

        for error in errors {
            let message = error.to_string();
            assert!(
                !message.contains('\u{1b}'),
                "unsanitized error: {message:?}"
            );
            assert!(
                message.contains("\\x1b"),
                "missing escaped control: {message:?}"
            );
        }
    }

    #[test]
    fn plan_includes_every_category_in_correct_order() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(tmp.path());
        let amq_root = tmp.path().join("amq");
        let claude_root = tmp.path().join("claude");
        let config = fixture_config(amq_root.clone(), &[("claude", claude_root)]);
        let worktree = tmp.path().join("worktrees/audit-x");
        let session = fixture_session("sid-1", "audit-x", &worktree);

        let plan = plan_for_session(&session, &paths, &config).expect("plan");

        // Order: Worktree, ProviderDir(claude), AmqInbox, LogRedact, SqliteRow.
        assert_eq!(plan.items.len(), 5);
        assert!(matches!(plan.items[0], PurgeItem::Worktree(_)));
        assert!(matches!(
            plan.items[1],
            PurgeItem::ProviderDir {
                provider: "claude",
                ..
            }
        ));
        assert!(matches!(plan.items[2], PurgeItem::AmqInbox(_)));
        assert!(matches!(plan.items[3], PurgeItem::LogScopedRedact { .. }));
        assert_eq!(plan.items[4], PurgeItem::SqliteRow);
    }

    #[test]
    fn plan_uses_runtime_amq_handle_and_configured_absolute_log_path() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(tmp.path());
        let amq_root = tmp.path().join("custom-state/amq");
        let log_path = tmp.path().join("external-logs/custom-dux.jsonl");
        let mut config = fixture_config(amq_root.clone(), &[]);
        config.log_path = log_path.clone();
        let worktree = paths.worktrees_root.join("runtime-handle");
        let session = fixture_session("sid-runtime", "feature/different", &worktree);
        let expected_inbox =
            resolve_for_containment(&amq_root.join("agents/runtime-handle")).unwrap();

        let plan = plan_for_session(&session, &paths, &config).expect("plan");
        assert!(plan.items.iter().any(|item| {
            matches!(
                item,
                PurgeItem::AmqInbox(path)
                    if path == &expected_inbox
            )
        }));
        assert!(plan.items.iter().any(|item| {
            matches!(
                item,
                PurgeItem::LogScopedRedact { path, .. } if path == &log_path
            )
        }));
        assert!(!plan.items.iter().any(|item| {
            matches!(item, PurgeItem::AmqInbox(path) if path.ends_with("feature-different"))
        }));
    }

    #[test]
    fn confirm_accepts_exact_phrase() {
        let plan = PurgePlan {
            session_id: "sid".to_string(),
            branch: "audit-x".to_string(),
            items: Vec::new(),
        };
        let mut reader = std::io::Cursor::new(b"PURGE audit-x\n".to_vec());
        assert!(confirm_with_reader(&plan, &mut reader).unwrap());
    }

    #[test]
    fn confirm_rejects_wrong_branch() {
        let plan = PurgePlan {
            session_id: "sid".to_string(),
            branch: "audit-x".to_string(),
            items: Vec::new(),
        };
        let mut reader = std::io::Cursor::new(b"PURGE other\n".to_vec());
        assert!(!confirm_with_reader(&plan, &mut reader).unwrap());
    }

    #[test]
    fn confirm_rejects_blank() {
        let plan = PurgePlan {
            session_id: "sid".to_string(),
            branch: "audit-x".to_string(),
            items: Vec::new(),
        };
        let mut reader = std::io::Cursor::new(b"\n".to_vec());
        assert!(!confirm_with_reader(&plan, &mut reader).unwrap());
    }

    #[test]
    fn redact_helper_rewrites_only_matching_lines() {
        let target = "abc";
        let line_match = r#"{"target":"dux::probe","fields":{"session_id":"abc","msg":"x"}}"#;
        let line_other = r#"{"target":"dux::probe","fields":{"session_id":"xyz","msg":"y"}}"#;
        let line_no_sid = r#"{"target":"dux::probe","fields":{"msg":"y"}}"#;

        let rewritten = maybe_redact_json_line(line_match, target).expect("match");
        let parsed: Value = serde_json::from_str(&rewritten).unwrap();
        assert_eq!(parsed["fields"]["redacted"], Value::Bool(true));
        assert!(parsed["fields"].get("session_id").is_none());
        assert!(parsed["fields"].get("msg").is_none());

        assert!(maybe_redact_json_line(line_other, target).is_none());
        assert!(maybe_redact_json_line(line_no_sid, target).is_none());
        assert!(maybe_redact_json_line("not json", target).is_none());
        assert!(maybe_redact_json_line("", target).is_none());
    }
}
