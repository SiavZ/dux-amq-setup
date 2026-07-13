//! Honest GDPR purge planning for isolated sessions and shared workspaces.
//!
//! This module implements audit02 Phase 10 (P0-J / GDPR Art 17
//! right-to-erasure). Today's `dux config reset --all` removes worktrees
//! and the sqlite database holistically but cannot target a single
//! session, and never touches the per-session provider chat history under
//! `<provider_root>/projects/<encoded>/` or the AMQ inbox under
//! `<amq_root>/agents/<agent_handle>/`. Shared provider history is path-scoped,
//! so a per-session purge must either retain the recovery row, explicitly accept
//! that residual, or remove the whole workspace history (including non-Dux chats).
//!
//! ## What gets purged
//!
//! Given a session identified by UUID, immutable handle, or an unambiguous
//! branch name, this module
//! removes:
//!
//! 1. An isolated session's worktree directory (refusing to touch anything
//!    outside the configured worktrees root — defence in depth against
//!    a malformed db row pointing at `/`, or overlapping a registered project).
//!    Shared sessions never remove their registered checkout.
//! 2. The Claude / Codex / Gemini chat-history dirs at
//!    `<provider_root>/<provider>/projects/<encoded>` where
//!    `<encoded>` is computed by `crate::purge_encoding`. Shared per-session
//!    purge reports this category incomplete unless residual data is accepted
//!    or a workspace-wide purge is confirmed.
//! 3. The session's AMQ inbox at `<amq_root>/agents/<agent_handle>` (we only
//!    delete it after exact `{store_id, session_id}` verification under the
//!    shared lock; peers' inboxes must remain untouched).
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
//! Worktree (isolated only) → providers → AMQ → log redact → sqlite. The row is
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

use crate::config::{Config, DuxPaths};
use crate::model::AgentSession;
use crate::purge_encoding;
use crate::sanitize;
use crate::storage::{SessionStore, load_store_id};
use crate::{git, peer};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration the purge cascade cannot derive from `DuxPaths` alone:
/// provider/AMQ roots, logs, and the protected registered-project inventory.
#[derive(Clone, Debug)]
pub struct PurgeConfig {
    /// Per-provider chat-history roots. Each entry maps a logical
    /// provider name (`"claude"`, `"codex"`, `"gemini"`) to the
    /// directory whose `projects/<encoded>` subtree should be purged.
    /// On the dux-amq VM these default to `/data/state/<provider>`.
    pub provider_data_dirs: Vec<(String, PathBuf)>,
    /// Root of the AMQ file-bus. The session's immutable-handle inbox
    /// lives at `<amq_root>/agents/<agent_handle>`. Defaults to
    /// `/data/state/amq`.
    pub amq_root: PathBuf,
    /// Resolved live log prefix (absolute or relative-to-Dux root exactly as
    /// the logger resolves it). Rotated siblings share this prefix.
    pub log_path: PathBuf,
    /// Complete registered-project inventory used by the whole-worktree guard.
    pub registered_project_paths: Vec<PathBuf>,
}

impl PurgeConfig {
    /// Build a sensible default `PurgeConfig` for the production layout
    /// described in `dux-amq/README.md`. Tests override this with a
    /// scratch directory.
    pub fn default_layout(paths: &DuxPaths, config: &Config) -> Result<Self> {
        let state = match std::env::var_os("STATE_ROOT") {
            Some(root) => {
                let root = PathBuf::from(root);
                if !root.is_absolute() {
                    bail!("STATE_ROOT must be an absolute path for hard purge");
                }
                root
            }
            None => paths
                .root
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| paths.root.clone()),
        };
        let amq_root =
            match std::env::var_os("AMQ_GLOBAL_ROOT").or_else(|| std::env::var_os("AM_ROOT")) {
                Some(root) => {
                    let root = PathBuf::from(root);
                    if !root.is_absolute() {
                        bail!("AMQ_GLOBAL_ROOT must be an absolute path for hard purge");
                    }
                    root
                }
                None => state.join("amq"),
            };
        Ok(Self {
            provider_data_dirs: vec![
                ("claude".to_string(), state.join("claude")),
                ("codex".to_string(), state.join("codex")),
                ("gemini".to_string(), state.join("gemini")),
            ],
            amq_root,
            log_path: crate::logger::resolve_log_path(&config.logging, paths),
            registered_project_paths: crate::config::registered_project_paths(config)?,
        })
    }
}

/// Operator consent for provider history shared by every session at one
/// canonical workspace path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SharedPurgeMode {
    /// Preserve the durable row and report provider history as incomplete.
    #[default]
    RetainIdentity,
    /// Delete provably session-owned records and the row while explicitly
    /// acknowledging that shared provider transcripts remain.
    AcceptResidualData,
    /// Delete provider history and purge every Dux session sharing the path.
    WorkspaceWide,
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
    /// Provider history cannot be attributed to one shared session.
    SharedProviderHistory {
        provider: &'static str,
        path: PathBuf,
        residual_accepted: bool,
    },
    /// Exact-owner delete of `<amq_root>/agents/<agent_handle>`. The parent
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
            Self::SharedProviderHistory {
                provider,
                path,
                residual_accepted,
            } => {
                let status = if *residual_accepted {
                    "RESIDUAL ACCEPTED"
                } else {
                    "INCOMPLETE"
                };
                format!(
                    "{status}: {provider} shared-workspace chat history retained at {}",
                    safe_path(path)
                )
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

/// Resolve `target` by session id, immutable handle, or an unambiguous branch
/// and build the safe default cascade plan.
pub fn build_plan(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
    target: &str,
) -> Result<PurgePlan> {
    build_plans_for_target(
        storage,
        paths,
        config,
        target,
        SharedPurgeMode::RetainIdentity,
    )?
    .into_iter()
    .next()
    .ok_or_else(|| anyhow!("purge target resolved to no sessions"))
}

/// Build one target plan, or every shared-workspace sibling plan when the
/// operator explicitly chose a workspace-wide provider-history purge.
pub fn build_plans_for_target(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
    target: &str,
    shared_mode: SharedPurgeMode,
) -> Result<Vec<PurgePlan>> {
    let sessions = storage
        .load_sessions_including_deleted()
        .context("failed to load sessions for purge planning")?;
    let session = resolve_target(&sessions, target)?;
    if shared_mode == SharedPurgeMode::AcceptResidualData && !session.shared_workspace() {
        bail!("--accept-residual-data applies only to a shared-workspace target");
    }
    if shared_mode == SharedPurgeMode::WorkspaceWide {
        if !session.shared_workspace() {
            bail!("--workspace-wide-provider-history requires a shared-workspace target");
        }
        let workspace = resolve_for_containment(Path::new(&session.worktree_path))?;
        return sessions
            .iter()
            .filter_map(|candidate| {
                match resolve_for_containment(Path::new(&candidate.worktree_path)) {
                    Ok(candidate_path) if candidate_path == workspace => Some(Ok(candidate)),
                    Ok(_) => None,
                    Err(err) => Some(Err(err)),
                }
            })
            .map(|candidate| {
                plan_for_session_with_mode(
                    candidate?,
                    paths,
                    config,
                    SharedPurgeMode::WorkspaceWide,
                )
            })
            .collect();
    }
    Ok(vec![plan_for_session_with_mode(
        session,
        paths,
        config,
        shared_mode,
    )?])
}

fn resolve_target<'a>(sessions: &'a [AgentSession], target: &str) -> Result<&'a AgentSession> {
    if let Some(session) = sessions.iter().find(|session| session.id == target) {
        return Ok(session);
    }
    if let Some(session) = sessions
        .iter()
        .find(|session| session.agent_handle() == target)
    {
        return Ok(session);
    }
    let mut branches = sessions
        .iter()
        .filter(|session| session.branch_name == target);
    let Some(session) = branches.next() else {
        bail!(
            "no session found matching id, agent handle, or branch {:?}",
            sanitize::for_terminal(target)
        );
    };
    if branches.next().is_some() {
        bail!(
            "branch {:?} matches more than one session; retry with a session UUID or agent_handle",
            sanitize::for_terminal(target)
        );
    }
    Ok(session)
}

/// Lower-level helper used by `build_plan` and by the `purge_all`
/// fan-out. Caller has already resolved the session.
pub fn plan_for_session(
    session: &AgentSession,
    paths: &DuxPaths,
    config: &PurgeConfig,
) -> Result<PurgePlan> {
    plan_for_session_with_mode(session, paths, config, SharedPurgeMode::RetainIdentity)
}

fn plan_for_session_with_mode(
    session: &AgentSession,
    paths: &DuxPaths,
    config: &PurgeConfig,
    shared_mode: SharedPurgeMode,
) -> Result<PurgePlan> {
    let mut items = Vec::new();

    // 1. Worktree. Shared sessions point at the user's real checkout and must
    // never offer or execute whole-workspace deletion.
    if !session.shared_workspace() && shared_mode != SharedPurgeMode::WorkspaceWide {
        let worktree = validate_delete_target(
            &paths.worktrees_root,
            Path::new(&session.worktree_path),
            "worktree",
        )?;
        git::guard_whole_workspace_removal(&worktree, &config.registered_project_paths)?;
        items.push(PurgeItem::Worktree(worktree));
    }

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
        if session.shared_workspace() && shared_mode != SharedPurgeMode::WorkspaceWide {
            items.push(PurgeItem::SharedProviderHistory {
                provider: provider_static,
                path,
                residual_accepted: shared_mode == SharedPurgeMode::AcceptResidualData,
            });
        } else {
            items.push(PurgeItem::ProviderDir {
                provider: provider_static,
                path,
            });
        }
    }

    // 3. AMQ inbox, keyed only by the immutable handle. Execution performs
    // exact {store_id, session_id} owner verification under the AMQ lock.
    let handle = session.agent_handle();
    if !handle.is_empty() {
        let agents_root = config.amq_root.join("agents");
        let inbox = validate_delete_target(&agents_root, &agents_root.join(handle), "AMQ inbox")?;
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
    paths: &DuxPaths,
    config: &PurgeConfig,
    dry_run: bool,
) -> Result<PurgeReport> {
    let sessions = storage
        .load_sessions_including_deleted()
        .context("failed to reload complete session identity before purge")?;
    let session = sessions
        .iter()
        .find(|session| session.id == plan.session_id)
        .ok_or_else(|| anyhow!("purge session row disappeared before execution"))?;
    let store_id = plan
        .items
        .iter()
        .any(|item| matches!(item, PurgeItem::AmqInbox(_)))
        .then(|| load_store_id(&paths.root))
        .transpose()
        .context("failed to load durable store identity before purge")?;
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
            execute_item(item, storage, session, config, store_id.as_deref(), dry_run)
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
    session: &AgentSession,
    config: &PurgeConfig,
    store_id: Option<&str>,
    dry_run: bool,
) -> PurgeOutcome {
    match item {
        PurgeItem::Worktree(p) => {
            match git::guard_whole_workspace_removal(p, &config.registered_project_paths) {
                Ok(()) => execute_remove_dir(p, dry_run),
                Err(err) => PurgeOutcome::Error(format!("{err:#}")),
            }
        }
        PurgeItem::ProviderDir { path, .. } => execute_remove_dir(path, dry_run),
        PurgeItem::SharedProviderHistory {
            residual_accepted, ..
        } => {
            if *residual_accepted {
                PurgeOutcome::Skipped(
                    "operator explicitly accepted residual shared provider history".to_string(),
                )
            } else {
                PurgeOutcome::Error(
                    "provider history is shared with sibling and non-Dux conversations; retry with --accept-residual-data or --workspace-wide-provider-history"
                        .to_string(),
                )
            }
        }
        PurgeItem::AmqInbox(path) => {
            execute_amq_inbox(path, &config.amq_root, store_id, session, dry_run)
        }
        PurgeItem::LogScopedRedact { path, .. } => execute_redact_logs(path, &session.id, dry_run),
        PurgeItem::SqliteRow => execute_delete_row(storage, &session.id, dry_run),
    }
}

fn execute_amq_inbox(
    path: &Path,
    root: &Path,
    store_id: Option<&str>,
    session: &AgentSession,
    dry_run: bool,
) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PurgeOutcome::Skipped(format!("{} does not exist", safe_path(path)));
        }
        Err(err) => return PurgeOutcome::Error(format!("{err:#}")),
    }
    let expected = root.join("agents").join(session.agent_handle());
    let matches_expected = resolve_for_containment(path)
        .and_then(|path| resolve_for_containment(&expected).map(|expected| path == expected));
    match matches_expected {
        Ok(true) => {}
        Ok(false) => {
            return PurgeOutcome::Error(
                "AMQ purge item does not match the session's immutable handle".to_string(),
            );
        }
        Err(err) => return PurgeOutcome::Error(format!("{err:#}")),
    }
    let Some(store_id) = store_id else {
        return PurgeOutcome::Error("durable store identity is unavailable".to_string());
    };
    match peer::free_amq_handle_at_root(root, store_id, session) {
        Ok(()) => PurgeOutcome::Done,
        Err(err) => PurgeOutcome::Error(format!("{err:#}")),
    }
}

fn execute_remove_dir(path: &Path, dry_run: bool) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PurgeOutcome::Skipped(format!("{} does not exist", safe_path(path)));
        }
        Err(err) => {
            return PurgeOutcome::Error(format!("inspect {}: {err}", safe_path(path)));
        }
    }
    match fs::remove_dir_all(path) {
        Ok(()) => PurgeOutcome::Done,
        Err(err) => PurgeOutcome::Error(format!("remove_dir_all({}): {err}", safe_path(path))),
    }
}

fn execute_delete_row(storage: &SessionStore, session_id: &str, dry_run: bool) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    match storage.delete_session(session_id) {
        Ok(()) => PurgeOutcome::Done,
        Err(err) => PurgeOutcome::Error(format!(
            "delete_session({}): {err}",
            sanitize::for_terminal(session_id)
        )),
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
/// purge-all`. Returns plans in load order and one operator-facing warning for
/// each malformed row retained because a complete safe plan was impossible.
/// Shared provider history always remains incomplete until the operator uses
/// the explicitly confirmed workspace-wide single-target flow.
pub fn build_plans_for_all(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
) -> Result<(Vec<PurgePlan>, Vec<String>)> {
    let sessions = storage
        .load_sessions_including_deleted()
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
                    "session {:?} (branch {:?}) has unsafe purge targets and was retained: {}",
                    sanitize::for_terminal(&session.id),
                    sanitize::for_terminal(&session.branch_name),
                    sanitize::for_terminal(&format!("{err:#}")),
                ));
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
            agent_handle: crate::model::normalize_agent_handle(id),
            shared_workspace: false,
            deleted_at: None,
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
            registered_project_paths: Vec::new(),
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
    fn plan_targets_deconflicted_agent_handle_not_worktree_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(tmp.path());
        let amq_root = tmp.path().join("custom-state/amq");
        let log_path = tmp.path().join("external-logs/custom-dux.jsonl");
        let mut config = fixture_config(amq_root.clone(), &[]);
        config.log_path = log_path.clone();
        let worktree = paths.worktrees_root.join("runtime-handle");
        let mut session = fixture_session("sid-runtime", "feature/different", &worktree);
        session.agent_handle = "runtime-handle-2".to_string();
        let expected_inbox =
            resolve_for_containment(&amq_root.join("agents/runtime-handle-2")).unwrap();

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
            matches!(item, PurgeItem::AmqInbox(path) if path.ends_with("runtime-handle"))
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
