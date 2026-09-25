//! Honest GDPR hard purge (Art. 17 right to erasure) for one agent, or all.
//!
//! Ported from the fork's `src/purge.rs` (3e5c1c32, be7bf48e, 15519969,
//! 8cfe2ee5, 0b831554, 4b27a744). `dux config reset --all` wipes everything
//! at once but cannot target one agent, and never touches the agent's provider
//! chat history or its AMQ inbox. This module can.
//!
//! ## What gets purged
//!
//! Given an agent identified by session id, immutable `agent_handle`, or an
//! unambiguous branch name:
//!
//! 1. A managed, isolated agent's worktree directory. Refused when it resolves
//!    outside the managed worktrees root, IS that root, or overlaps a
//!    registered project in either direction (the central
//!    [`git::guard_whole_workspace_removal`]). A shared-workspace agent (the
//!    project's own checkout) and a standalone agent (the user's folder) never
//!    offer this step: that directory is the user's.
//! 2. Provider chat history at `<provider_root>/projects/<encoded>`, where
//!    `<encoded>` is [`crate::purge_encoding::encode_str`] of the agent's
//!    directory. For a directory the agent does not own alone (shared checkout
//!    or user folder), history is path-scoped and may hold sibling or non-dux
//!    chats, so a per-agent purge reports it INCOMPLETE unless the operator
//!    explicitly accepts the residual or confirms a workspace-wide purge.
//! 3. The AMQ inbox at `<amq_root>/agents/<agent_handle>`, released only after
//!    exact `{store_id, session_id}` owner verification under the shared lock,
//!    so a peer's inbox is never touched.
//! 4. The agent's startup-command logs under `<dux home>/startup-command-logs`.
//! 5. Log records mentioning this agent, redacted in place in the live log and
//!    every rotated copy (plain, or `.gz`). A dux text line naming the session
//!    id keeps its timestamp and level and loses its message; a JSON line whose
//!    `fields.session_id` matches has its `fields` replaced with
//!    `{"redacted": true}`. The audit trail (something was purged, when)
//!    survives; the content does not.
//! 6. The session row and its dependent rows, LAST.
//!
//! ## Fail closed
//!
//! Planning resolves every path symlink-aware and refuses relative paths,
//! `..`, symlink escapes and root targets. `execute` re-runs the whole-worktree
//! guard at removal time. If any step errors, the row is retained so the
//! operator can re-run the purge and it resumes cleanly: a crash or failure
//! never leaves orphaned bytes with no record of whose they were.
//!
//! ## Dry run
//!
//! Every step honors `dry_run`; the integration tests assert it per category.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::config::{Config, DuxPaths};
use crate::git;
use crate::model::AgentSession;
use crate::purge_encoding;
use crate::sanitize;
use crate::storage::{SessionStore, load_store_id};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// What the purge cascade cannot derive from `DuxPaths` alone.
#[derive(Clone, Debug)]
pub struct PurgeConfig {
    /// Per-provider chat-history roots: `(provider name, root)`. The purge
    /// targets `<root>/projects/<encoded>`.
    pub provider_data_dirs: Vec<(String, PathBuf)>,
    /// Root of the AMQ file bus; inboxes live at `<amq_root>/agents/<handle>`.
    pub amq_root: PathBuf,
    /// Resolved live log path, exactly as the logger resolves it. Rotated
    /// siblings (`<name>.N`, `<name>.N.gz`) share this prefix.
    pub log_path: PathBuf,
    /// Every registered project checkout, for the whole-worktree guard.
    pub registered_project_paths: Vec<PathBuf>,
}

impl PurgeConfig {
    /// The production layout.
    ///
    /// - Claude history: `$CLAUDE_CONFIG_DIR`, else `$STATE_ROOT/claude`, else
    ///   `~/.claude`.
    /// - Codex and Gemini history: only under `$STATE_ROOT` (the dux-amq VM
    ///   overlay mounts them there in the same `projects/<encoded>` shape).
    ///   Outside that layout neither CLI keys history by project directory, so
    ///   there is no per-agent directory to name.
    /// - AMQ: `$AMQ_GLOBAL_ROOT`, else `$AM_ROOT`, else `$STATE_ROOT/amq`, else
    ///   an `amq` directory beside the dux home.
    ///
    /// Every override must be absolute: a relative root would resolve against
    /// the purge's working directory and delete the wrong tree.
    pub fn default_layout(
        paths: &DuxPaths,
        config: &Config,
        registered_project_paths: Vec<PathBuf>,
    ) -> Result<Self> {
        let state = absolute_env("STATE_ROOT")?;
        let claude = match absolute_env("CLAUDE_CONFIG_DIR")? {
            Some(dir) => Some(dir),
            None => match &state {
                Some(state) => Some(state.join("claude")),
                None => home::home_dir().map(|home| home.join(".claude")),
            },
        };
        let mut provider_data_dirs = Vec::new();
        if let Some(claude) = claude {
            provider_data_dirs.push(("claude".to_string(), claude));
        }
        if let Some(state) = &state {
            provider_data_dirs.push(("codex".to_string(), state.join("codex")));
            provider_data_dirs.push(("gemini".to_string(), state.join("gemini")));
        }
        let amq_root = match absolute_env("AMQ_GLOBAL_ROOT")? {
            Some(root) => root,
            None => match absolute_env("AM_ROOT")? {
                Some(root) => root,
                None => match &state {
                    Some(state) => state.join("amq"),
                    None => paths
                        .root
                        .parent()
                        .map(|parent| parent.join("amq"))
                        .unwrap_or_else(|| paths.root.join("amq")),
                },
            },
        };
        Ok(Self {
            provider_data_dirs,
            amq_root,
            log_path: crate::logger::resolve_log_path(&config.logging, paths),
            registered_project_paths,
        })
    }
}

fn absolute_env(name: &str) -> Result<Option<PathBuf>> {
    match std::env::var_os(name) {
        None => Ok(None),
        Some(raw) if raw.is_empty() => Ok(None),
        Some(raw) => {
            let path = PathBuf::from(raw);
            if !path.is_absolute() {
                bail!("{name} must be an absolute path for hard purge");
            }
            Ok(Some(path))
        }
    }
}

/// Load the config for a destructive command, failing closed.
///
/// The ordinary [`crate::config::load_config`] recovers from a bad file by
/// falling back to defaults, which is right for startup and wrong here: a
/// purge or reset run against a defaulted config would see no registered
/// projects and so protect none of them. A missing file is fine (nothing
/// registered in it); an unreadable or invalid one is an error.
pub fn load_config_strict(paths: &DuxPaths) -> Result<Config> {
    let raw = match fs::read_to_string(&paths.config_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read {}", safe_path(&paths.config_path)));
        }
    };
    let mut doc = raw
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", safe_path(&paths.config_path)))?;
    crate::config_migrate::apply_load_migrations(&mut doc)
        .with_context(|| format!("failed to migrate {}", safe_path(&paths.config_path)))?;
    toml::from_str::<Config>(&doc.to_string()).with_context(|| {
        format!(
            "{} does not parse as a dux config",
            safe_path(&paths.config_path)
        )
    })
}

/// The complete protected-checkout inventory: every project in the config, in
/// the database, and every agent's own recorded project path (so a row whose
/// project was dropped from the list is still covered). Fails closed on one
/// that does not expand to a safe absolute path.
pub fn protected_project_paths(
    config: &Config,
    store: &SessionStore,
    sessions: &[AgentSession],
) -> Result<Vec<PathBuf>> {
    let stored = store
        .load_projects()
        .context("failed to load the registered project inventory")?;
    git::registered_project_paths(
        config
            .projects
            .iter()
            .map(|project| project.path.as_str())
            .chain(stored.iter().map(|project| project.path.as_str()))
            .chain(sessions.iter().filter_map(|session| session.project_path())),
    )
}

/// Operator consent for provider history keyed by a directory the target does
/// not own alone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SharedPurgeMode {
    /// Keep the row and report provider history as incomplete.
    #[default]
    RetainIdentity,
    /// Delete what this agent provably owns, and the row, explicitly
    /// acknowledging that the shared provider transcripts remain.
    AcceptResidualData,
    /// Delete the directory's provider history and purge every agent sharing it.
    WorkspaceWide,
}

/// One unit of work in a [`PurgePlan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeItem {
    /// The session row and its dependent rows.
    SqliteRow,
    /// Recursive delete of a managed worktree.
    Worktree(PathBuf),
    /// Recursive delete of `<provider_root>/projects/<encoded>`.
    ProviderDir {
        provider: &'static str,
        path: PathBuf,
    },
    /// Provider history that cannot be attributed to this agent alone.
    SharedProviderHistory {
        provider: &'static str,
        path: PathBuf,
        residual_accepted: bool,
    },
    /// Exact-owner delete of `<amq_root>/agents/<agent_handle>`. The parent
    /// `agents/` dir is never removed.
    AmqInbox(PathBuf),
    /// Recursive delete of this agent's startup-command log directory.
    StartupLogs(PathBuf),
    /// Streaming rewrite of the live log and its rotated copies.
    LogScopedRedact {
        /// Informational: the agent's creation time, recorded so the audit
        /// trail shows the cut-over point. Every record is visited regardless.
        since: DateTime<Utc>,
        path: PathBuf,
    },
}

impl PurgeItem {
    /// Operator-facing one-line description, for the confirmation prompt and
    /// the dry-run report. Every path is sanitized for the terminal.
    pub fn describe(&self) -> String {
        match self {
            Self::SqliteRow => "session row in sessions.sqlite3".to_string(),
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
            Self::StartupLogs(p) => format!("startup-command logs {}", safe_path(p)),
            Self::LogScopedRedact { since, path } => format!(
                "redact {} records since {}",
                safe_path(path),
                since.to_rfc3339()
            ),
        }
    }
}

/// A complete plan for purging exactly one agent.
#[derive(Clone, Debug)]
pub struct PurgePlan {
    pub session_id: String,
    /// The agent's branch, or its handle for a standalone agent, which has no
    /// branch. This is also what the operator types to confirm.
    pub branch: String,
    pub items: Vec<PurgeItem>,
}

/// Per-item outcome reported by [`execute`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    Done,
    /// A no-op, such as a directory that was already absent.
    Skipped(String),
    DryRun,
    Error(String),
}

/// The audit record returned by [`execute`], one entry per item in order.
#[derive(Clone, Debug)]
pub struct PurgeReport {
    pub session_id: String,
    pub branch: String,
    pub entries: Vec<(PurgeItem, PurgeOutcome)>,
    pub dry_run: bool,
}

impl PurgeReport {
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

    /// Whether any item ended in `Error`; the CLI exits non-zero on it.
    pub fn had_errors(&self) -> bool {
        self.entries
            .iter()
            .any(|(_, o)| matches!(o, PurgeOutcome::Error(_)))
    }
}

// ---------------------------------------------------------------------------
// Plan construction
// ---------------------------------------------------------------------------

/// Resolve `target` and build the safe default plan.
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

/// Build one target plan, or one plan per agent sharing the target's
/// directory when the operator explicitly chose a workspace-wide purge.
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
    if shared_mode == SharedPurgeMode::AcceptResidualData && !directory_is_not_owned(session) {
        bail!("--accept-residual-data applies only to a shared-workspace target");
    }
    if shared_mode == SharedPurgeMode::WorkspaceWide {
        if !directory_is_not_owned(session) {
            bail!("--workspace-wide-provider-history requires a shared-workspace target");
        }
        let workspace = resolve_for_containment(Path::new(session.directory()))?;
        return sessions
            .iter()
            .filter_map(|candidate| {
                match resolve_for_containment(Path::new(candidate.directory())) {
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

/// Session id first, then immutable handle, then a branch that names exactly
/// one agent. An ambiguous branch refuses rather than guessing.
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
        .filter(|session| session.branch_name() == Some(target));
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

/// Whether the agent's directory is someone else's too: a shared checkout, or
/// a standalone agent's folder. Such a directory is never removed and its
/// path-scoped provider history cannot be attributed to this agent alone.
fn directory_is_not_owned(session: &AgentSession) -> bool {
    session.shared_workspace() || session.managed_worktree().is_none()
}

fn plan_label(session: &AgentSession) -> String {
    session
        .branch_name()
        .unwrap_or_else(|| session.agent_handle())
        .to_string()
}

/// Build the default plan for an already resolved agent.
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
    let not_owned = directory_is_not_owned(session);

    // 1. Worktree, for a managed isolated agent only.
    if !not_owned && let Some(worktree) = session.managed_worktree() {
        let worktree =
            validate_delete_target(&paths.worktrees_root, Path::new(worktree), "worktree")?;
        git::guard_whole_workspace_removal(&worktree, &config.registered_project_paths)?;
        items.push(PurgeItem::Worktree(worktree));
    }

    // 2. Provider history, encoded from the agent's absolute directory. A
    // relative or non-UTF-8 path refuses: mis-encoding would silently orphan
    // the chat-history directory the purge claims to have erased.
    let directory = session.directory();
    let encoded = purge_encoding::encode_str(directory).with_context(|| {
        format!(
            "failed to encode directory {:?} for provider history lookup",
            sanitize::for_terminal(directory),
        )
    })?;
    for (provider_name, root) in &config.provider_data_dirs {
        let provider = static_provider_name(provider_name);
        let provider_projects = root.join("projects");
        let path = validate_delete_target(
            &provider_projects,
            &provider_projects.join(&encoded),
            "provider history",
        )?;
        if not_owned && shared_mode != SharedPurgeMode::WorkspaceWide {
            items.push(PurgeItem::SharedProviderHistory {
                provider,
                path,
                residual_accepted: shared_mode == SharedPurgeMode::AcceptResidualData,
            });
        } else {
            items.push(PurgeItem::ProviderDir { provider, path });
        }
    }

    // 3. AMQ inbox, keyed only by the immutable handle, never the branch or
    // the directory basename (either can drift or be pathlike).
    let handle = session.agent_handle();
    if !handle.is_empty() {
        let agents_root = config.amq_root.join("agents");
        let inbox = validate_delete_target(&agents_root, &agents_root.join(handle), "AMQ inbox")?;
        items.push(PurgeItem::AmqInbox(inbox));
    }

    // 4. Startup-command logs, which only a managed agent can have.
    if let Some(project_id) = session.project_id() {
        let logs_root = paths.root.join(crate::startup::LOG_ROOT);
        let dir = crate::startup::agent_log_dir(paths, project_id, &session.id);
        items.push(PurgeItem::StartupLogs(validate_delete_target(
            &logs_root,
            &dir,
            "startup-command logs",
        )?));
    }

    // 5. Log redaction.
    items.push(PurgeItem::LogScopedRedact {
        since: session.created_at,
        path: config.log_path.clone(),
    });

    // 6. The row, LAST.
    items.push(PurgeItem::SqliteRow);

    Ok(PurgePlan {
        session_id: session.id.clone(),
        branch: plan_label(session),
        items,
    })
}

/// Resolve an existing path through symlinks, or resolve its deepest existing
/// ancestor and append the missing suffix, so an already-missing target can be
/// validated without weakening containment. Relative paths and `..` refuse.
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

/// The containment check every recursive delete target passes: resolved
/// strictly inside the resolved root, never the root itself.
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

/// Map a configured provider name to a `&'static str` label, so arbitrary
/// user-supplied strings never flow into the item type.
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

/// Execute the cascade. `dry_run` turns every destructive step into a
/// `DryRun` outcome. The row step is skipped (retained for retry) when any
/// earlier step errored.
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
    crate::logger::info(&format!(
        "purge: cascade starting for session {safe_session_id} ({} items, dry_run={dry_run})",
        plan.items.len()
    ));

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
        // Never log the item's path content for a redacted session beyond the
        // sanitized description; the session id itself is what the redaction
        // pass looks for, so it is not written here after the redact step.
        match &outcome {
            PurgeOutcome::Error(why) => crate::logger::error(&format!(
                "purge: step failed: {}: {}",
                item.describe(),
                sanitize::for_terminal(why)
            )),
            PurgeOutcome::Skipped(why) => crate::logger::info(&format!(
                "purge: step skipped: {}: {}",
                item.describe(),
                sanitize::for_terminal(why)
            )),
            PurgeOutcome::DryRun | PurgeOutcome::Done => {
                crate::logger::info(&format!("purge: step complete: {}", item.describe()))
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
            if dry_run {
                return PurgeOutcome::DryRun;
            }
            remove_whole_worktree(p, session.project_path(), &config.registered_project_paths)
        }
        PurgeItem::ProviderDir { path, .. } | PurgeItem::StartupLogs(path) => {
            execute_remove_dir(path, dry_run)
        }
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

/// The ONE place a purge removes a whole worktree. The protected-workspace
/// guard runs again here, at removal time, so a project registered between
/// planning and execution still stops it. When the owning repository is known
/// its now-dangling worktree registration is pruned afterwards (best effort).
fn remove_whole_worktree(
    path: &Path,
    project_path: Option<&str>,
    protected: &[PathBuf],
) -> PurgeOutcome {
    if let Err(err) = git::guard_whole_workspace_removal(path, protected) {
        return PurgeOutcome::Error(format!("{err:#}"));
    }
    let outcome = execute_remove_dir(path, false);
    if matches!(outcome, PurgeOutcome::Done)
        && let Some(repo) = project_path
    {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "prune"])
            .output();
    }
    outcome
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
    // The item must name this session's handle exactly; a plan built for one
    // agent can never free another's inbox.
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
    // Through the peer router's exact-owner free (stops the wake, then removes).
    match crate::purge_amq::free_amq_handle_at_root(root, store_id, session) {
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
// Log redaction
// ---------------------------------------------------------------------------

/// Rewrite the live log and every rotated copy, redacting records that name
/// `session_id`. Each file is staged in a sibling temp file and renamed into
/// place, so a crash mid-rewrite leaves the original intact. `Done` even when
/// nothing matched: the audit fact is that the rewrite ran.
fn execute_redact_logs(log_path: &Path, session_id: &str, dry_run: bool) -> PurgeOutcome {
    let Some(log_root) = log_path.parent() else {
        return PurgeOutcome::Error(format!("log path has no parent: {}", safe_path(log_path)));
    };
    match fs::metadata(log_root) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return PurgeOutcome::Error(format!(
                "log root {} is not a directory",
                safe_path(log_root)
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return PurgeOutcome::Skipped(format!(
                "log root {} does not exist",
                safe_path(log_root)
            ));
        }
        Err(err) => {
            return PurgeOutcome::Error(format!("inspect log root {}: {err}", safe_path(log_root)));
        }
    }

    let log_files = match collect_log_files(log_path) {
        Ok(v) => v,
        Err(e) => return PurgeOutcome::Error(format!("scan {}: {e:#}", safe_path(log_root))),
    };
    if log_files.is_empty() {
        return PurgeOutcome::Skipped(format!(
            "no {} log files under {}",
            log_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(sanitize::for_terminal)
                .unwrap_or_else(|| "configured".to_string()),
            safe_path(log_root)
        ));
    }
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    for file in &log_files {
        if let Err(e) = redact_one_log_file(file, session_id) {
            return PurgeOutcome::Error(format!("redact {} failed: {e:#}", safe_path(file)));
        }
    }
    PurgeOutcome::Done
}

/// The live log plus its rotated siblings `<name>.N` and `<name>.N.gz`. Flat
/// only: recursing could touch unrelated files a user keeps nearby.
fn collect_log_files(log_path: &Path) -> Result<Vec<PathBuf>> {
    let root = log_path
        .parent()
        .ok_or_else(|| anyhow!("log path has no parent: {}", safe_path(log_path)))?;
    let prefix = log_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("log filename is not UTF-8: {}", safe_path(log_path)))?;
    let rotated_prefix = format!("{prefix}.");
    let mut out = Vec::new();
    for entry in fs::read_dir(root).with_context(|| format!("read_dir {}", safe_path(root)))? {
        let entry = entry.with_context(|| format!("read_dir entry under {}", safe_path(root)))?;
        let ty = entry
            .file_type()
            .with_context(|| format!("file_type {}", safe_path(&entry.path())))?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let is_log = name == prefix
            || name.strip_prefix(&rotated_prefix).is_some_and(|rest| {
                rest.strip_suffix(".gz")
                    .unwrap_or(rest)
                    .parse::<u32>()
                    .is_ok()
            });
        if ty.is_file() && is_log {
            out.push(entry.path());
        }
    }
    out.sort();
    Ok(out)
}

/// Rewrite one log file in place (through a temp file and a rename), gzip
/// aware. Returns the count of redacted lines.
fn redact_one_log_file(path: &Path, session_id: &str) -> Result<usize> {
    let gz = path.extension().is_some_and(|ext| ext == "gz");
    let mut tmp_name = path.as_os_str().to_os_string();
    tmp_name.push(format!(".purge.{}.tmp", std::process::id()));
    let tmp_path = PathBuf::from(tmp_name);

    let result = (|| -> Result<usize> {
        let src = fs::File::open(path).with_context(|| format!("open {}", safe_path(path)))?;
        let reader: Box<dyn Read> = if gz {
            Box::new(flate2::read::GzDecoder::new(src))
        } else {
            Box::new(src)
        };
        let mut reader = BufReader::new(reader);
        let tmp = fs::File::create(&tmp_path)
            .with_context(|| format!("create {}", safe_path(&tmp_path)))?;
        crate::file_modes::restrict_to_owner_best_effort(&tmp_path, "redacted log file");
        let mut writer: Box<dyn Write> = if gz {
            Box::new(flate2::write::GzEncoder::new(
                tmp,
                flate2::Compression::default(),
            ))
        } else {
            Box::new(tmp)
        };

        let mut redacted = 0usize;
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader
                .read_until(b'\n', &mut line)
                .with_context(|| format!("read {}", safe_path(path)))?;
            if n == 0 {
                break;
            }
            let had_newline = line.ends_with(b"\n");
            let payload = if had_newline {
                &line[..line.len() - 1]
            } else {
                &line[..]
            };
            // A line that is not UTF-8 cannot be a dux record naming this id
            // in a way we can rewrite safely, but it can still CONTAIN the id:
            // check the bytes, and replace the whole line when it does.
            let rewritten: Option<Vec<u8>> = match std::str::from_utf8(payload) {
                Ok(text) => maybe_redact_line(text, session_id).map(String::into_bytes),
                Err(_) => contains_bytes(payload, session_id.as_bytes())
                    .then(|| b"[redacted: purged session]".to_vec()),
            };
            match rewritten {
                Some(new_line) => {
                    redacted += 1;
                    writer.write_all(&new_line)?;
                }
                None => writer.write_all(payload)?,
            }
            if had_newline {
                writer.write_all(b"\n")?;
            }
        }
        writer.flush()?;
        drop(writer);
        fs::File::open(&tmp_path)
            .and_then(|f| f.sync_all())
            .with_context(|| format!("sync {}", safe_path(&tmp_path)))?;
        fs::rename(&tmp_path, path)
            .with_context(|| format!("rename {} -> {}", safe_path(&tmp_path), safe_path(path)))?;
        Ok(redacted)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Redact one log line if it names `target`, else `None`.
///
/// A JSON Lines record matches on `fields.session_id` and keeps everything but
/// `fields`. Any other line (dux's own `<rfc3339> <LEVEL> <message>` text
/// format) matches when it contains the id at all, and keeps its timestamp and
/// level so the audit trail still shows that something happened then.
fn maybe_redact_line(line: &str, target: &str) -> Option<String> {
    if line.trim().is_empty() || target.is_empty() {
        return None;
    }
    if line.trim_start().starts_with('{') {
        if let Some(rewritten) = maybe_redact_json_line(line, target) {
            return Some(rewritten);
        }
        if serde_json::from_str::<Value>(line).is_ok() && !line.contains(target) {
            return None;
        }
    }
    if !line.contains(target) {
        return None;
    }
    let mut parts = line.splitn(3, ' ');
    match (parts.next(), parts.next()) {
        (Some(ts), Some(level))
            if DateTime::parse_from_rfc3339(ts).is_ok()
                && matches!(level, "ERROR" | "WARN" | "INFO" | "DEBUG") =>
        {
            Some(format!("{ts} {level:<5} [redacted: purged session]"))
        }
        _ => Some("[redacted: purged session]".to_string()),
    }
}

/// If `line` is a JSON object whose `fields.session_id` equals `target`,
/// return it with `fields` replaced by `{"redacted": true}`.
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
        .is_some_and(|sid| sid == target);
    if !matches {
        return None;
    }
    let mut redacted = Map::new();
    redacted.insert("redacted".to_string(), Value::Bool(true));
    obj.insert("fields".to_string(), Value::Object(redacted));
    serde_json::to_string(&parsed).ok()
}

// ---------------------------------------------------------------------------
// Confirmation
// ---------------------------------------------------------------------------

/// Read one line and check it is exactly `PURGE <branch>`. Typing the branch
/// name is the point: it prevents accidental yes-mashing.
pub fn confirm_with_reader<R: BufRead + ?Sized>(plan: &PurgePlan, reader: &mut R) -> Result<bool> {
    let mut input = String::new();
    reader
        .read_line(&mut input)
        .context("failed to read confirmation from stdin")?;
    Ok(input.trim() == format!("PURGE {}", plan.branch))
}

/// Print the plan to stderr, ask for the phrase, and read stdin.
pub fn confirm_interactive(plan: &PurgePlan) -> Result<bool> {
    eprintln!(
        "Will permanently delete the following for session {} (branch {}):",
        sanitize::for_terminal(&plan.session_id),
        sanitize::for_terminal(&plan.branch)
    );
    for item in &plan.items {
        eprintln!("  - {}", item.describe());
    }
    eprint!(
        "Type 'PURGE {}' to confirm: ",
        sanitize::for_terminal(&plan.branch)
    );
    let _ = std::io::stderr().flush();
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    confirm_with_reader(plan, &mut handle)
}

// ---------------------------------------------------------------------------
// Bulk purge-all
// ---------------------------------------------------------------------------

/// Plans for every agent, tombstones included, plus one sanitized warning per
/// row retained because a complete safe plan was impossible. A malformed row
/// never aborts the others and is never guessed at.
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
            Err(err) => failures.push(format!(
                "session {:?} (branch {:?}) has unsafe purge targets and was retained: {}",
                sanitize::for_terminal(&session.id),
                sanitize::for_terminal(&plan_label(session)),
                sanitize::for_terminal(&format!("{err:#}")),
            )),
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
    use crate::model::{
        AgentWorkspace, BranchProvenance, ManagedWorkspace, ProviderKind, SessionStatus,
    };

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
            agent_handle: crate::model::normalize_agent_handle(id),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: format!("{id}-slot"),
            provider: ProviderKind::new("claude"),
            workspace: AgentWorkspace::Managed(ManagedWorkspace {
                project_id: "proj".to_string(),
                project_path: None,
                source_branch: "main".to_string(),
                branch_name: branch.to_string(),
                initial_branch: branch.to_string(),
                branch_provenance: BranchProvenance::CreatedByDux,
                worktree_path: worktree.to_string_lossy().to_string(),
            }),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
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

        // Worktree, ProviderDir(claude), AmqInbox, StartupLogs, LogRedact, SqliteRow.
        assert_eq!(plan.items.len(), 6);
        assert!(matches!(plan.items[0], PurgeItem::Worktree(_)));
        assert!(matches!(
            plan.items[1],
            PurgeItem::ProviderDir {
                provider: "claude",
                ..
            }
        ));
        assert!(matches!(plan.items[2], PurgeItem::AmqInbox(_)));
        assert!(matches!(plan.items[3], PurgeItem::StartupLogs(_)));
        assert!(matches!(plan.items[4], PurgeItem::LogScopedRedact { .. }));
        assert_eq!(plan.items[5], PurgeItem::SqliteRow);
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

    /// Upstream's logger writes `<rfc3339> <LEVEL> <message>` text, not JSON:
    /// a line naming the session loses its message and keeps its timestamp
    /// and level; other lines are untouched.
    #[test]
    fn redact_rewrites_dux_text_log_lines_naming_the_session() {
        let ts = "2026-09-24T18:00:00.000000+00:00";
        let hit = format!("{ts} INFO  started agent sid-9 in /home/ada/secret-project");
        let miss = format!("{ts} INFO  started agent sid-10 elsewhere");
        assert_eq!(
            maybe_redact_line(&hit, "sid-9").as_deref(),
            Some(format!("{ts} INFO  [redacted: purged session]").as_str())
        );
        assert!(maybe_redact_line(&miss, "sid-9").is_none());
        assert_eq!(
            maybe_redact_line("free text with sid-9", "sid-9").as_deref(),
            Some("[redacted: purged session]")
        );
        // A JSON record naming a different session in fields but this one in
        // its message still loses the line: the content is what is erased.
        let json_mention = r#"{"fields":{"session_id":"other","msg":"peer sid-9 said hi"}}"#;
        assert!(maybe_redact_line(json_mention, "sid-9").is_some());
    }

    /// Rotated copies are redacted too, including gzipped ones, and a file
    /// that merely shares the prefix is left alone.
    #[test]
    fn redact_covers_rotated_and_gzipped_copies_only() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().join("dux.log");
        fs::write(&live, "2026-01-01T00:00:00+00:00 INFO  sid-g live\n").unwrap();
        fs::write(
            tmp.path().join("dux.log.1"),
            "2026-01-01T00:00:00+00:00 INFO  sid-g rotated\n",
        )
        .unwrap();
        let gz_path = tmp.path().join("dux.log.2.gz");
        {
            let mut enc = flate2::write::GzEncoder::new(
                fs::File::create(&gz_path).unwrap(),
                flate2::Compression::default(),
            );
            enc.write_all(b"2026-01-01T00:00:00+00:00 WARN  sid-g compressed\n")
                .unwrap();
            enc.finish().unwrap();
        }
        let unrelated = tmp.path().join("dux.log.backup-notes");
        fs::write(&unrelated, "sid-g keep\n").unwrap();

        assert_eq!(
            execute_redact_logs(&live, "sid-g", false),
            PurgeOutcome::Done
        );

        assert!(!fs::read_to_string(&live).unwrap().contains("sid-g"));
        assert!(
            !fs::read_to_string(tmp.path().join("dux.log.1"))
                .unwrap()
                .contains("sid-g")
        );
        let mut text = String::new();
        flate2::read::GzDecoder::new(fs::File::open(&gz_path).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        assert!(!text.contains("sid-g"), "{text}");
        assert!(text.contains("WARN"), "timestamp and level survive: {text}");
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "sid-g keep\n");
    }
}
