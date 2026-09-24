//! Each provider's own id for an agent's conversation: capturing it at launch,
//! resolving it for jcode, and a one-time recovery of histories stranded under
//! old per-agent worktrees.
//!
//! Why: a provider's "resume the latest conversation here" selector
//! (`claude --continue`, `codex resume --last`) picks by recency within a
//! directory, not by dux agent. Recording the provider's id per agent lets a
//! relaunch resume exactly that agent's conversation. The id is persisted in
//! `agent_sessions.provider_session_ids` (see `SessionStore`).
//!
//! - Claude accepts an id at launch (`--session-id`), so dux assigns one.
//! - Codex has no such flag, so dux snapshots its rollout files before the
//!   launch and identifies the one new rollout for the agent's directory.
//! - jcode records every session's working directory in its own metadata
//!   database, so dux resolves the newest non-trivial session there.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use chrono::DateTime;
use notify::{RecursiveMode, Watcher};
use rustix::fs::{CWD, RenameFlags, renameat_with};
use serde_json::Value;
use uuid::Uuid;

use crate::config::ProviderCommandConfig;
use crate::model::{AgentSession, Project, normalize_agent_handle};
use crate::storage::SessionStore;

/// Whether `session` runs in a directory it shares with other agents. Every
/// shared-only rule in this module (never resume-latest, the Claude transcript
/// check) reads this.
pub fn session_is_shared(session: &AgentSession) -> bool {
    session.shared_workspace()
}

/// The agent's stable handle, used to match a stranded history's old worktree
/// directory name to the agent it belonged to.
pub fn agent_handle(session: &AgentSession) -> String {
    session.agent_handle.clone()
}

/// Escape control characters so an id or path read from a provider's files
/// cannot drive the terminal when it lands in a status line or log.
fn printable(value: &str) -> String {
    value.escape_default().to_string()
}

/// How one launch reaches its provider conversation, beyond upstream's
/// resume-latest flag (`AgentLaunchRequest::resume`).
///
/// Decided on the engine thread (`Engine::provider_session_launch`, or
/// [`fresh_capture_for`] for a brand-new agent) and carried on the launch
/// request, so the worker that spawns the PTY does exactly what was decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ProviderSessionLaunch {
    /// Upstream behaviour: `interactive_args(resume)`, nothing recorded.
    #[default]
    Plain,
    /// Resume exactly this conversation through `resume_by_id_args`.
    ResumeId(String),
    /// Fresh Claude launch: the worker assigns a UUID with `--session-id` and
    /// reports it once the PTY is up, so the next launch can resume it.
    CaptureClaude,
    /// Fresh Codex launch: the worker snapshots the rollout directory first and
    /// reports the one new rollout for this directory once Codex writes it.
    CaptureCodex,
}

impl ProviderSessionLaunch {
    pub fn is_resume_id(&self) -> bool {
        matches!(self, Self::ResumeId(_))
    }
}

/// The capture a FRESH launch of `provider` should do, or `Plain` when there is
/// nothing to record: only Claude and Codex are captured at launch (jcode is
/// resolved from its own metadata instead), and only when the provider is
/// configured to resume by id, since an id nothing can use is not worth the
/// rollout scan.
pub fn fresh_capture_for(provider: &str, config: &ProviderCommandConfig) -> ProviderSessionLaunch {
    if !config.supports_session_resume_by_id() {
        return ProviderSessionLaunch::Plain;
    }
    match provider {
        "claude" => ProviderSessionLaunch::CaptureClaude,
        "codex" => ProviderSessionLaunch::CaptureCodex,
        _ => ProviderSessionLaunch::Plain,
    }
}

/// Resume ids flow into provider CLI arguments, so only ids whose shape dux
/// recognizes are accepted: UUIDs for every provider except jcode, whose native
/// `session_<name>_<epoch>_<hex>` ids are validated structurally.
pub fn provider_resume_id_is_valid(provider: &str, id: &str) -> bool {
    if provider == "jcode" {
        jcode_session_id_is_valid(id)
    } else {
        Uuid::parse_str(id).is_ok()
    }
}

/// Everything [`plan_resume_id`] needs, gathered by the engine so the rule
/// itself stays a pure function a test can drive with any combination.
pub struct ResumeIdInput<'a> {
    pub provider: &'a str,
    pub config: &'a ProviderCommandConfig,
    /// The id dux recorded for this agent and provider, if any.
    pub stored_id: Option<&'a str>,
    /// Whether the agent shares its directory with other agents.
    pub shared: bool,
    pub directory: &'a Path,
    /// Whether this provider has run for this agent before.
    pub started: bool,
    /// Whether another agent runs the same provider in the same directory.
    pub directory_has_sibling: bool,
}

/// The conversation id a relaunch should resume, or `None` to fall back to
/// upstream's resume-latest rule (or a fresh start).
///
/// - A stored id wins when its shape is valid and the provider resumes by id.
///   A shared Claude agent additionally needs the transcript to still exist in
///   the directory's Claude project, because `claude --resume` of a missing id
///   silently starts a blank conversation (36568df7).
/// - jcode has no capture at launch, but its metadata records each session's
///   directory, so when this agent is the only jcode agent in its directory the
///   newest non-trivial session there is unambiguously its own (eb2d2eee).
pub fn plan_resume_id(
    input: &ResumeIdInput<'_>,
    claude_target_exists: impl Fn(&Path, &str) -> bool,
    latest_jcode: impl Fn(&Path) -> Option<String>,
) -> Option<String> {
    if !input.config.supports_session_resume_by_id() {
        return None;
    }
    if let Some(id) = input.stored_id
        && provider_resume_id_is_valid(input.provider, id)
        && (!input.shared
            || input.provider != "claude"
            || claude_target_exists(input.directory, id))
    {
        return Some(id.to_string());
    }
    if input.provider == "jcode" && input.started && !input.directory_has_sibling {
        return latest_jcode(input.directory).filter(|id| jcode_session_id_is_valid(id));
    }
    None
}

/// What a worker prepared for a fresh launch before spawning it.
pub enum PreparedCapture {
    None,
    /// The UUID handed to `claude --session-id`.
    Claude(String),
    /// The rollout snapshot taken before Codex starts.
    Codex(CodexCapture),
}

impl PreparedCapture {
    pub fn claude_session_id(&self) -> Option<&str> {
        match self {
            Self::Claude(id) => Some(id),
            Self::None | Self::Codex(_) => None,
        }
    }

    /// Release anything held for a launch that did not happen.
    pub fn abort(self) {
        if let Self::Codex(capture) = self {
            capture.abort();
        }
    }
}

/// Prepare the capture `launch` asks for, before the PTY is spawned.
///
/// Nothing is written here: a Claude replacement id is only recorded once its
/// launch succeeds, so a failed spawn keeps the previous (still resumable) id.
pub fn prepare_fresh_capture(
    launch: &ProviderSessionLaunch,
    cwd: &Path,
    roots: &ProviderDataRoots,
) -> Result<PreparedCapture> {
    match launch {
        ProviderSessionLaunch::CaptureClaude => {
            Ok(PreparedCapture::Claude(Uuid::new_v4().to_string()))
        }
        ProviderSessionLaunch::CaptureCodex => Ok(CodexCapture::begin(cwd, &roots.codex_sessions)?
            .map_or(PreparedCapture::None, PreparedCapture::Codex)),
        ProviderSessionLaunch::Plain | ProviderSessionLaunch::ResumeId(_) => {
            Ok(PreparedCapture::None)
        }
    }
}

/// The argv a launch runs with.
///
/// `resume` is upstream's resume-latest flag. A targeted resume replaces the
/// base args with `resume_by_id_args`; Codex also gets `-C <cwd>`, because
/// `codex resume <id>` otherwise reopens the conversation in the directory it
/// was recorded in, which for a moved agent is not where dux launched it.
pub fn launch_args(
    config: &ProviderCommandConfig,
    provider: &str,
    launch: &ProviderSessionLaunch,
    resume: bool,
    fresh_claude_id: Option<&str>,
    cwd: &Path,
) -> Result<Vec<String>> {
    match launch {
        ProviderSessionLaunch::ResumeId(session_id) => {
            let mut args = config
                .resume_by_id_args(session_id)
                .context("targeted session resume is not configured")?;
            if provider == "codex" {
                args.extend(["-C".to_string(), cwd.to_string_lossy().into_owned()]);
            }
            Ok(args)
        }
        ProviderSessionLaunch::Plain
        | ProviderSessionLaunch::CaptureClaude
        | ProviderSessionLaunch::CaptureCodex => {
            let mut args = config.interactive_args(resume);
            if let Some(session_id) = fresh_claude_id {
                args.extend(["--session-id".to_string(), session_id.to_string()]);
            }
            Ok(args)
        }
    }
}

const MAX_TRANSCRIPT_FILES: usize = 100_000;
const MAX_JSON_LINE_BYTES: usize = 16 * 1024 * 1024;
const CODEX_PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct ProviderDataRoots {
    pub claude_projects: PathBuf,
    pub codex_sessions: PathBuf,
}

impl ProviderDataRoots {
    pub fn from_home() -> Result<Self> {
        let home = home::home_dir().context("home directory is unavailable")?;
        Ok(Self {
            claude_projects: home.join(".claude/projects"),
            codex_sessions: home.join(".codex/sessions"),
        })
    }
}

/// Minimum size for a jcode session file to count as resumable. A pane that
/// spawned fresh and sat idle leaves a near-empty session (~1-3 KB); real
/// working sessions start around 50 KB. Resuming an idle shell would hide
/// the session the operator actually wants back.
const MIN_RESUMABLE_JCODE_SESSION_BYTES: u64 = 4096;

/// How many newest metadata rows to consider per worktree. Bounds the
/// filesystem probes when a directory has accumulated many stray sessions.
const MAX_JCODE_RESUME_CANDIDATES: usize = 10;

/// True iff `id` has the exact shape of a jcode session id
/// (`session_<alnum>_<epoch-ms>_<hex>`, e.g.
/// `session_cactus_1788156095921_18c33bc3e9ed4d80`). jcode ids are not
/// UUIDs, and the id flows into a `--resume` CLI argument, so the shape
/// check doubles as an injection guard.
pub fn jcode_session_id_is_valid(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("session_") else {
        return false;
    };
    let mut parts = rest.split('_');
    let (Some(name), Some(epoch), Some(hex), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric())
        && !epoch.is_empty()
        && epoch.chars().all(|c| c.is_ascii_digit())
        && !hex.is_empty()
        && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolve the newest resumable jcode session for `worktree` from jcode's
/// own metadata database (`recent_sessions` in
/// `~/.jcode/session-metadata-v1.sqlite3`). dux cannot assign jcode ids at
/// spawn the way it does for Claude, and jcode has no rollout files to
/// diff the way Codex does — but every jcode session records its
/// `working_dir`, and dux agents own their directory, so "newest non-empty
/// session for this directory" is the session the operator would pick from
/// jcode's `/resume` menu. Any failure resolves to `None`, which keeps the
/// fresh-launch behavior.
pub fn resolve_latest_jcode_session(
    metadata_db: &Path,
    sessions_dir: &Path,
    worktree: &Path,
) -> Option<String> {
    let conn = rusqlite::Connection::open_with_flags(
        metadata_db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    let canonical = fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
    let mut stmt = conn
        .prepare(
            "SELECT session_id FROM recent_sessions WHERE working_dir IN (?1, ?2) \
             ORDER BY updated_at_ms DESC",
        )
        .ok()?;
    let ids = stmt
        .query_map(
            rusqlite::params![worktree.to_string_lossy(), canonical.to_string_lossy()],
            |row| row.get::<_, String>(0),
        )
        .ok()?;
    for id in ids.flatten().take(MAX_JCODE_RESUME_CANDIDATES) {
        if !jcode_session_id_is_valid(&id) {
            continue;
        }
        let session_file = sessions_dir.join(format!("{id}.json"));
        match fs::metadata(&session_file) {
            Ok(meta) if meta.len() >= MIN_RESUMABLE_JCODE_SESSION_BYTES => return Some(id),
            _ => continue,
        }
    }
    None
}

/// [`resolve_latest_jcode_session`] against the real `~/.jcode` layout.
pub fn resolve_latest_jcode_session_from_home(worktree: &Path) -> Option<String> {
    let root = home::home_dir()?.join(".jcode");
    resolve_latest_jcode_session(
        &root.join("session-metadata-v1.sqlite3"),
        &root.join("sessions"),
        worktree,
    )
}

#[derive(Clone, Default)]
pub struct CodexCaptureCoordinator {
    inner: Arc<(Mutex<HashMap<PathBuf, CaptureState>>, Condvar)>,
}

#[derive(Clone)]
enum CaptureState {
    Capturing,
    Blocked(String),
}

pub struct CodexCapture {
    coordinator: CodexCaptureCoordinator,
    cwd: PathBuf,
    sessions_root: PathBuf,
    known_ids: HashSet<String>,
    active: bool,
}

impl std::fmt::Debug for CodexCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexCapture")
            .field("cwd", &self.cwd)
            .field("known_id_count", &self.known_ids.len())
            .finish_non_exhaustive()
    }
}

fn capture_coordinator() -> &'static CodexCaptureCoordinator {
    static COORDINATOR: OnceLock<CodexCaptureCoordinator> = OnceLock::new();
    COORDINATOR.get_or_init(CodexCaptureCoordinator::default)
}

impl CodexCapture {
    /// Snapshot the rollouts that exist before a fresh Codex launch in `cwd`.
    ///
    /// `Ok(None)` when another capture already owns this directory (or one
    /// failed there earlier): with two uncaptured Codex launches in one
    /// directory, the new rollout could belong to either, so this launch goes
    /// uncaptured rather than guessing. It never waits for the other capture.
    /// The fork blocked here instead, which on upstream's launch worker would
    /// hold a tab's launch until the OTHER agent's first turn.
    pub fn begin(cwd: impl AsRef<Path>, sessions_root: &Path) -> Result<Option<Self>> {
        Self::begin_with(capture_coordinator().clone(), cwd, sessions_root)
    }

    fn begin_with(
        coordinator: CodexCaptureCoordinator,
        cwd: impl AsRef<Path>,
        sessions_root: &Path,
    ) -> Result<Option<Self>> {
        let cwd = fs::canonicalize(cwd.as_ref()).with_context(|| {
            format!(
                "failed to canonicalize Codex launch cwd {}",
                cwd.as_ref().display()
            )
        })?;
        {
            let (state_lock, _) = &*coordinator.inner;
            let mut states = state_lock.lock().expect("Codex capture mutex poisoned");
            match states.get(&cwd) {
                None => {
                    states.insert(cwd.clone(), CaptureState::Capturing);
                }
                Some(CaptureState::Capturing) => return Ok(None),
                Some(CaptureState::Blocked(reason)) => {
                    crate::logger::warn(&format!(
                        "Codex capture skipped for {}: {}",
                        printable(&cwd.to_string_lossy()),
                        printable(reason),
                    ));
                    return Ok(None);
                }
            }
        }

        // Only the ids matter here, and each lives on its rollout's first line,
        // so the snapshot never reads a rollout past that line.
        match codex_rollout_paths(sessions_root) {
            Ok(paths) => Ok(Some(Self {
                coordinator,
                cwd,
                sessions_root: sessions_root.to_path_buf(),
                known_ids: paths
                    .into_iter()
                    .filter_map(parse_codex_rollout_identity)
                    .map(|(id, _)| id)
                    .collect(),
                active: true,
            })),
            Err(err) => {
                release_capture(&coordinator, &cwd);
                Err(err)
            }
        }
    }

    pub fn wait_for_id(
        &self,
        timeout: Option<Duration>,
        process_id: Option<u32>,
    ) -> Result<Option<String>> {
        fs::create_dir_all(&self.sessions_root).with_context(|| {
            format!(
                "failed to create Codex sessions directory {}",
                self.sessions_root.display()
            )
        })?;
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = event_tx.send(event);
        })
        .context("failed to start Codex rollout watcher")?;
        watcher
            .watch(&self.sessions_root, RecursiveMode::Recursive)
            .with_context(|| {
                format!(
                    "failed to watch Codex sessions directory {}",
                    self.sessions_root.display()
                )
            })?;

        let started = Instant::now();
        let mut paths = codex_rollout_paths(&self.sessions_root)?;
        loop {
            let mut candidates = std::mem::take(&mut paths)
                .into_iter()
                .filter_map(parse_codex_rollout_identity)
                .filter(|(id, _)| !self.known_ids.contains(id))
                .filter(|(_, cwd)| canonical_or_raw(cwd) == self.cwd)
                .map(|(id, _)| id)
                .collect::<HashSet<_>>();
            match candidates.len() {
                1 => return Ok(candidates.drain().next()),
                n if n > 1 => bail!("multiple new Codex rollout candidates matched the workspace"),
                _ => {}
            }

            if process_id.is_some_and(|pid| !process_is_running(pid)) {
                return Ok(None);
            }

            let remaining = match timeout {
                Some(timeout) => Some(
                    timeout
                        .checked_sub(started.elapsed())
                        .context("timed out waiting for a new Codex rollout")?,
                ),
                None => None,
            };
            let event = match (remaining, process_id) {
                (None, None) => event_rx
                    .recv()
                    .context("Codex rollout watcher stopped unexpectedly")?,
                (remaining, process_id) => {
                    let wait = remaining
                        .unwrap_or(CODEX_PROCESS_POLL_INTERVAL)
                        .min(CODEX_PROCESS_POLL_INTERVAL);
                    match event_rx.recv_timeout(wait) {
                        Ok(event) => event,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if process_id.is_some_and(|pid| !process_is_running(pid)) {
                                return Ok(None);
                            }
                            if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
                                bail!("timed out waiting for a new Codex rollout");
                            }
                            continue;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            bail!("Codex rollout watcher stopped unexpectedly")
                        }
                    }
                }
            }
            .context("Codex rollout watcher failed")?;
            paths = codex_rollout_paths_from_events(event.paths)?;
        }
    }

    pub fn resolve(mut self) {
        release_capture(&self.coordinator, &self.cwd);
        self.active = false;
    }

    pub fn abort(mut self) {
        release_capture(&self.coordinator, &self.cwd);
        self.active = false;
    }

    pub fn block(mut self, reason: &str) {
        let (state_lock, wake) = &*self.coordinator.inner;
        let mut states = state_lock.lock().expect("Codex capture mutex poisoned");
        states.insert(self.cwd.clone(), CaptureState::Blocked(reason.to_string()));
        wake.notify_all();
        self.active = false;
    }
}

fn codex_rollout_paths(root: &Path) -> Result<Vec<PathBuf>> {
    recursive_files(root, is_codex_rollout_path)
}

fn codex_rollout_paths_from_events(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut rollouts = Vec::new();
    for path in paths {
        if is_codex_rollout_path(&path) {
            rollouts.push(path);
        } else if path.is_dir() {
            rollouts.extend(codex_rollout_paths(&path)?);
        }
    }
    Ok(rollouts)
}

fn is_codex_rollout_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn parse_codex_rollout_identity(path: PathBuf) -> Option<(String, PathBuf)> {
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.file_type().is_file() {
        return None;
    }
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut line = Vec::new();
    let read = reader.read_until(b'\n', &mut line).ok()?;
    if read == 0 || line.len() > MAX_JSON_LINE_BYTES {
        return None;
    }
    let first = serde_json::from_slice::<Value>(&line).ok()?;
    if first.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = first.get("payload")?;
    // Prefer the resumable THREAD id over the rollout's own id.
    //
    // When a codex session is forked, `payload.id` is the sub-agent's own
    // identifier and `codex resume <that id>` fails with:
    //
    //   thread/resume failed: cannot resume an unloaded multi-agent v2
    //   sub-agent through its parent; resume the parent first (code -32600)
    //
    // `payload.session_id` is the thread codex will actually resume, and
    // equals `parent_thread_id` on a forked rollout. On a non-forked rollout
    // the two are identical, so preferring `session_id` is a no-op there.
    // Rollouts written by older codex builds omit `session_id` entirely, so
    // fall back to `id` for those.
    let id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .or_else(|| payload.get("id").and_then(Value::as_str))?
        .to_string();
    let cwd = PathBuf::from(payload.get("cwd")?.as_str()?);
    if Uuid::parse_str(&id).is_err() || !cwd.is_absolute() {
        return None;
    }
    Some((id, cwd))
}

fn process_is_running(process_id: u32) -> bool {
    i32::try_from(process_id)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .is_some_and(|pid| rustix::process::test_kill_process(pid).is_ok())
}

impl Drop for CodexCapture {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let (state_lock, wake) = &*self.coordinator.inner;
        let mut states = state_lock.lock().expect("Codex capture mutex poisoned");
        states.insert(
            self.cwd.clone(),
            CaptureState::Blocked("capture ended before its rollout was identified".to_string()),
        );
        wake.notify_all();
    }
}

fn release_capture(coordinator: &CodexCaptureCoordinator, cwd: &Path) {
    let (state_lock, wake) = &*coordinator.inner;
    let mut states = state_lock.lock().expect("Codex capture mutex poisoned");
    states.remove(cwd);
    wake.notify_all();
}

#[derive(Clone, Debug)]
pub struct ProviderSessionUpdate {
    pub session_id: String,
    pub provider: String,
    pub provider_session_id: String,
}

#[derive(Default, Debug)]
pub struct RecoveryReport {
    pub updates: Vec<ProviderSessionUpdate>,
    pub copied_artifacts: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
struct RecoveryIdentity {
    session_id: String,
    provider: String,
    agent_handle: String,
    project_name: String,
    destination_cwd: PathBuf,
}

#[derive(Clone, Debug)]
struct Transcript {
    provider: String,
    id: String,
    cwd: PathBuf,
    activity_ms: i64,
    artifact: Option<PathBuf>,
}

#[derive(Default)]
struct RecoveryMatches {
    assignments: HashMap<(String, String), Vec<Transcript>>,
    ambiguous: HashSet<(String, String)>,
    warnings: Vec<String>,
}

pub fn recover_stranded_histories(
    sessions: &[AgentSession],
    projects: &[Project],
    worktrees_root: &Path,
    roots: &ProviderDataRoots,
    store: &SessionStore,
) -> Result<RecoveryReport> {
    let identities = recovery_identities(sessions, projects, store);
    if identities.is_empty() {
        return Ok(RecoveryReport::default());
    }
    let mut transcripts = scan_claude_transcripts(&roots.claude_projects)?;
    transcripts.extend(scan_codex_rollouts(&roots.codex_sessions)?);
    let mut matched = match_recovery_candidates(&identities, &transcripts, worktrees_root);
    let mut report = RecoveryReport {
        warnings: std::mem::take(&mut matched.warnings),
        ..RecoveryReport::default()
    };

    for identity in identities {
        let key = (identity.session_id.clone(), identity.provider.clone());
        if matched.ambiguous.contains(&key) {
            continue;
        }
        let Some(candidates) = matched.assignments.remove(&key) else {
            continue;
        };
        if identity.provider == "claude" {
            let destination = roots
                .claude_projects
                .join(encode_claude_project_dir(&identity.destination_cwd));
            let mut copy_failed = false;
            for candidate in &candidates {
                let Some(artifact) = &candidate.artifact else {
                    continue;
                };
                match copy_claude_artifacts(artifact, &destination) {
                    Ok(copied) => report.copied_artifacts += copied,
                    Err(err) => {
                        report.warnings.push(format!(
                            "Could not safely copy Claude history for session \"{}\"; leaving its UUID unmapped: {err:#}",
                            printable(&identity.session_id),
                        ));
                        copy_failed = true;
                        break;
                    }
                }
            }
            if copy_failed {
                continue;
            }
        }
        let Some(latest) = candidates
            .iter()
            .max_by_key(|candidate| candidate.activity_ms)
        else {
            continue;
        };
        match store.set_provider_session_id_if_missing(
            &identity.session_id,
            &identity.provider,
            &latest.id,
        ) {
            Ok(true) => report.updates.push(ProviderSessionUpdate {
                session_id: identity.session_id,
                provider: identity.provider,
                provider_session_id: latest.id.clone(),
            }),
            Ok(false) => {}
            Err(err) => report.warnings.push(format!(
                "Could not persist recovered {} UUID for session \"{}\": {err:#}",
                printable(&identity.provider),
                printable(&identity.session_id),
            )),
        }
    }
    Ok(report)
}

/// Whether any agent could gain an id from [`recover_stranded_histories`]:
/// the cheap, synchronous half of it, so a startup with nothing to recover
/// never scans the provider directories or waits on them.
pub fn recovery_has_candidates(
    sessions: &[AgentSession],
    projects: &[Project],
    store: &SessionStore,
) -> bool {
    !recovery_identities(sessions, projects, store).is_empty()
}

fn recovery_identities(
    sessions: &[AgentSession],
    projects: &[Project],
    store: &SessionStore,
) -> Vec<RecoveryIdentity> {
    let mut identities = Vec::new();
    for session in sessions {
        // A standalone agent has no project, so no historical worktree path
        // could ever have named it.
        let Some(project_id) = session.project_id() else {
            continue;
        };
        let Some(project) = projects.iter().find(|project| project.id == project_id) else {
            continue;
        };
        let Some(session_project_path) = session.workspace.project_path() else {
            continue;
        };
        if !paths_equivalent(Path::new(session_project_path), Path::new(&project.path)) {
            continue;
        }
        let stored = store.provider_session_ids(&session.id).unwrap_or_default();
        for provider in &session.started_providers {
            if !matches!(provider.as_str(), "claude" | "codex") || stored.contains_key(provider) {
                continue;
            }
            identities.push(RecoveryIdentity {
                session_id: session.id.clone(),
                provider: provider.clone(),
                agent_handle: agent_handle(session),
                project_name: project.name.clone(),
                destination_cwd: PathBuf::from(session.directory()),
            });
        }
    }
    identities
}

fn match_recovery_candidates(
    identities: &[RecoveryIdentity],
    transcripts: &[Transcript],
    worktrees_root: &Path,
) -> RecoveryMatches {
    let mut result = RecoveryMatches::default();
    for transcript in transcripts {
        let Some((project_name, handle)) =
            historical_worktree_identity(&transcript.cwd, worktrees_root)
        else {
            continue;
        };
        let matching = identities
            .iter()
            .filter(|identity| {
                identity.provider == transcript.provider
                    && identity.project_name == project_name
                    && identity.agent_handle == handle
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            if matching.len() > 1 {
                let candidate_ids = matching
                    .iter()
                    .map(|identity| printable(&identity.session_id))
                    .collect::<Vec<_>>()
                    .join(", ");
                for identity in &matching {
                    result
                        .ambiguous
                        .insert((identity.session_id.clone(), identity.provider.clone()));
                }
                result.warnings.push(format!(
                    "Refused ambiguous {} history recovery for project \"{}\" and agent \"{}\"; candidate sessions: {}.",
                    transcript.provider,
                    printable(&project_name),
                    printable(&handle),
                    candidate_ids,
                ));
            }
            continue;
        }
        let identity = matching[0];
        result
            .assignments
            .entry((identity.session_id.clone(), identity.provider.clone()))
            .or_default()
            .push(transcript.clone());
    }
    for key in &result.ambiguous {
        result.assignments.remove(key);
    }
    result
}

fn historical_worktree_identity(cwd: &Path, root: &Path) -> Option<(String, String)> {
    if !cwd.is_absolute() {
        return None;
    }
    let relative = cwd.strip_prefix(root).ok()?;
    let components = relative.components().collect::<Vec<_>>();
    if components.len() != 2
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let project = components[0].as_os_str().to_str()?.to_string();
    let handle = normalize_agent_handle(components[1].as_os_str().to_str()?);
    (!handle.is_empty()).then_some((project, handle))
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    canonical_or_raw(left) == canonical_or_raw(right)
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn scan_claude_transcripts(root: &Path) -> Result<Vec<Transcript>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    for project_dir in
        fs::read_dir(root).with_context(|| format!("failed to read {}", root.display()))?
    {
        let project_dir = project_dir?;
        if !project_dir.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(project_dir.path())? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            {
                paths.push(entry.path());
                if paths.len() > MAX_TRANSCRIPT_FILES {
                    bail!("Claude transcript file limit exceeded");
                }
            }
        }
    }
    Ok(paths
        .into_iter()
        .filter_map(parse_claude_transcript)
        .collect())
}

fn parse_claude_transcript(path: PathBuf) -> Option<Transcript> {
    let file_id = path.file_stem()?.to_str()?.to_string();
    if Uuid::parse_str(&file_id).is_err() {
        return None;
    }
    let mut ids = HashSet::new();
    let mut cwds = HashSet::new();
    let mut activity_ms = None;
    let fallback_ms = visit_jsonl(&path, |_, value| {
        if let Some(id) = value.get("sessionId").and_then(Value::as_str) {
            ids.insert(id.to_string());
        }
        if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
            cwds.insert(PathBuf::from(cwd));
        }
        activity_ms = activity_ms.max(json_timestamp_ms(value));
    })
    .ok()?;
    if ids.iter().any(|id| id != &file_id) || cwds.len() != 1 {
        return None;
    }
    Some(Transcript {
        provider: "claude".to_string(),
        id: file_id,
        cwd: cwds.into_iter().next()?,
        activity_ms: activity_ms.unwrap_or(fallback_ms),
        artifact: Some(path),
    })
}

fn scan_codex_rollouts(root: &Path) -> Result<Vec<Transcript>> {
    let paths = codex_rollout_paths(root)?;
    Ok(paths.into_iter().filter_map(parse_codex_rollout).collect())
}

fn parse_codex_rollout(path: PathBuf) -> Option<Transcript> {
    let (id, cwd) = parse_codex_rollout_identity(path.clone())?;
    let mut activity_ms = None;
    let fallback_ms = visit_jsonl(&path, |_, value| {
        activity_ms = activity_ms.max(json_timestamp_ms(value));
    })
    .ok()?;
    Some(Transcript {
        provider: "codex".to_string(),
        id,
        cwd,
        activity_ms: activity_ms.unwrap_or(fallback_ms),
        artifact: None,
    })
}

fn visit_jsonl(path: &Path, mut visit: impl FnMut(usize, &Value)) -> Result<i64> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        bail!("refusing non-regular transcript {}", path.display());
    }
    let fallback_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0);
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut line_index = 0;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_JSON_LINE_BYTES {
            bail!("transcript line exceeds safety limit in {}", path.display());
        }
        if let Ok(value) = serde_json::from_slice::<Value>(&line) {
            visit(line_index, &value);
        }
        line_index += 1;
    }
    Ok(fallback_ms)
}

fn json_timestamp_ms(value: &Value) -> Option<i64> {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("payload")
                .and_then(|payload| payload.get("timestamp"))
                .and_then(Value::as_str)
        })
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.timestamp_millis())
}

fn recursive_files(root: &Path, keep: impl Fn(&Path) -> bool) -> Result<Vec<PathBuf>> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut stack = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() && keep(&entry.path()) {
                files.push(entry.path());
                if files.len() > MAX_TRANSCRIPT_FILES {
                    bail!("provider transcript file limit exceeded");
                }
            }
        }
    }
    Ok(files)
}

pub fn encode_claude_project_dir(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let raw = raw
        .strip_suffix('/')
        .filter(|trimmed| !trimmed.is_empty())
        .unwrap_or(&raw);
    raw.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

pub fn claude_resume_target_exists(cwd: &Path, session_id: &str) -> bool {
    let Ok(roots) = ProviderDataRoots::from_home() else {
        return false;
    };
    let transcript = roots
        .claude_projects
        .join(encode_claude_project_dir(cwd))
        .join(format!("{session_id}.jsonl"));
    is_resumable_claude_transcript(&transcript)
}

fn is_resumable_claude_transcript(path: &Path) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut first_line = Vec::new();
    let Ok(read) = BufReader::new(file).read_until(b'\n', &mut first_line) else {
        return false;
    };
    read > 0
        && first_line.len() <= MAX_JSON_LINE_BYTES
        && serde_json::from_slice::<Value>(&first_line)
            .is_ok_and(|value| value.get("type").and_then(Value::as_str) != Some("bridge-session"))
}

fn copy_claude_artifacts(source_jsonl: &Path, destination_dir: &Path) -> Result<usize> {
    ensure_plain_directory(destination_dir)?;
    let mut copied = usize::from(atomic_copy(
        source_jsonl,
        &destination_dir.join(
            source_jsonl
                .file_name()
                .context("Claude transcript has no file name")?,
        ),
    )?);
    let companion = source_jsonl.with_extension("");
    match fs::symlink_metadata(&companion) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing symlink companion {}", companion.display());
        }
        Ok(metadata) if metadata.file_type().is_dir() => {
            copied += usize::from(atomic_copy(
                &companion,
                &destination_dir.join(companion.file_name().context("companion has no name")?),
            )?);
        }
        Ok(_) => bail!("refusing non-directory companion {}", companion.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(err).with_context(|| format!("failed to inspect {}", companion.display()));
        }
    }
    Ok(copied)
}

fn ensure_plain_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        bail!("refusing symlink destination {}", path.display());
    }
    Ok(())
}

fn atomic_copy(source: &Path, destination: &Path) -> Result<bool> {
    if destination.exists() {
        return Ok(false);
    }
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        bail!("refusing symlink source {}", source.display());
    }
    let parent = destination
        .parent()
        .context("copy destination has no parent")?;
    ensure_plain_directory(parent)?;
    let temp = parent.join(format!(".dux-resume-copy-{}", Uuid::new_v4()));
    let result = if metadata.is_file() {
        copy_file(source, &temp)
    } else if metadata.is_dir() {
        copy_directory(source, &temp)
    } else {
        bail!("refusing non-file artifact {}", source.display());
    };
    if let Err(err) = result {
        remove_temp(&temp);
        return Err(err);
    }
    match renameat_with(CWD, &temp, CWD, destination, RenameFlags::NOREPLACE) {
        Ok(()) => {
            File::open(parent)?.sync_all()?;
            Ok(true)
        }
        Err(err) if err == rustix::io::Errno::EXIST => {
            remove_temp(&temp);
            Ok(false)
        }
        Err(err) => {
            remove_temp(&temp);
            Err(std::io::Error::from(err)).with_context(|| {
                format!(
                    "failed to atomically install {} at {}",
                    source.display(),
                    destination.display()
                )
            })
        }
    }
}

fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;
    output.sync_all()?;
    fs::set_permissions(destination, fs::metadata(source)?.permissions())?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            bail!("refusing symlink in Claude companion directory");
        }
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if file_type.is_file() {
            copy_file(&entry.path(), &target)?;
        } else {
            bail!("refusing special file in Claude companion directory");
        }
    }
    fs::set_permissions(destination, fs::metadata(source)?.permissions())?;
    File::open(destination)?.sync_all()?;
    Ok(())
}

fn remove_temp(path: &Path) {
    if path.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::model::{
        AgentWorkspace, BranchProvenance, ManagedWorkspace, ProjectBranchStatus, ProviderKind,
        SessionStatus,
    };

    fn identity(id: &str) -> RecoveryIdentity {
        RecoveryIdentity {
            session_id: id.to_string(),
            provider: "claude".to_string(),
            agent_handle: "agent-one".to_string(),
            project_name: "project-one".to_string(),
            destination_cwd: PathBuf::from("/shared/project-one"),
        }
    }

    fn write_rollout(dir: &Path, name: &str, meta: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("{meta}\n")).unwrap();
        path
    }

    /// A forked codex session records the SUB-AGENT under `payload.id` and the
    /// resumable thread under `payload.session_id`. Storing `id` makes
    /// `codex resume <id>` fail with "cannot resume an unloaded multi-agent v2
    /// sub-agent through its parent", which presents in dux as an agent that
    /// silently refuses to start.
    #[test]
    fn codex_rollout_identity_prefers_thread_id_over_forked_subagent_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "rollout-2026-08-21T09-30-33-01a02303.jsonl",
            r#"{"type":"session_meta","payload":{"session_id":"01a00ed0-a960-7f10-9902-0bb44545408f","id":"01a02303-98ed-7ed0-bb20-7bdbd90393d9","forked_from_id":"01a00ed0-a960-7f10-9902-0bb44545408f","parent_thread_id":"01a00ed0-a960-7f10-9902-0bb44545408f","cwd":"/Users/dev/Projects/PuzzleBook"}}"#,
        );
        let (id, cwd) = parse_codex_rollout_identity(path).expect("identity");
        assert_eq!(
            id, "01a00ed0-a960-7f10-9902-0bb44545408f",
            "must record the resumable parent thread, not the sub-agent id",
        );
        assert_eq!(cwd, PathBuf::from("/Users/dev/Projects/PuzzleBook"));
    }

    /// Non-forked rollouts carry identical `session_id` and `id`, so preferring
    /// `session_id` must be a no-op there.
    #[test]
    fn codex_rollout_identity_unchanged_when_not_forked() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "rollout-2026-08-17T11-22-31-01a00ed0.jsonl",
            r#"{"type":"session_meta","payload":{"session_id":"01a00ed0-a960-7f10-9902-0bb44545408f","id":"01a00ed0-a960-7f10-9902-0bb44545408f","cwd":"/Users/dev/Projects/PuzzleBook"}}"#,
        );
        let (id, _) = parse_codex_rollout_identity(path).expect("identity");
        assert_eq!(id, "01a00ed0-a960-7f10-9902-0bb44545408f");
    }

    /// Older codex builds omit `session_id` entirely; those must still resolve
    /// via `id` rather than being dropped from recovery.
    #[test]
    fn codex_rollout_identity_falls_back_to_id_when_session_id_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rollout(
            dir.path(),
            "rollout-2026-01-01T00-00-00-legacy.jsonl",
            r#"{"type":"session_meta","payload":{"id":"019efa0b-925c-7660-8091-64af45e3e8e2","cwd":"/Users/dev/Projects/Legacy"}}"#,
        );
        let (id, cwd) = parse_codex_rollout_identity(path).expect("identity");
        assert_eq!(id, "019efa0b-925c-7660-8091-64af45e3e8e2");
        assert_eq!(cwd, PathBuf::from("/Users/dev/Projects/Legacy"));
    }

    /// A managed agent whose directory is `worktree` and whose project is
    /// checked out at `project_path`.
    fn managed_session(
        id: &str,
        provider: &str,
        project_path: &Path,
        worktree: &Path,
    ) -> AgentSession {
        let now = chrono::Utc::now();
        AgentSession {
            id: id.to_string(),
            slot_tab_id: format!("{id}-slot"),
            provider: ProviderKind::new(provider),
            workspace: AgentWorkspace::Managed(ManagedWorkspace {
                project_id: "project-one".to_string(),
                project_path: Some(project_path.to_string_lossy().to_string()),
                source_branch: "main".to_string(),
                branch_name: id.to_string(),
                initial_branch: id.to_string(),
                branch_provenance: BranchProvenance::CreatedByDux,
                worktree_path: worktree.to_string_lossy().to_string(),
            }),
            agent_handle: normalize_agent_handle(id),
            shared_workspace: false,
            deleted_at: None,
            title: None,
            started_providers: vec![provider.to_string()],
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    }

    fn project(name: &str, path: &Path) -> Project {
        Project {
            id: "project-one".to_string(),
            name: name.to_string(),
            path: path.to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::new("claude"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    #[test]
    fn ambiguous_recovery_is_refused() {
        let root = PathBuf::from("/state/dux/worktrees");
        let transcript = Transcript {
            provider: "claude".to_string(),
            id: Uuid::new_v4().to_string(),
            cwd: root.join("project-one/agent-one"),
            activity_ms: 1,
            artifact: None,
        };
        let matches =
            match_recovery_candidates(&[identity("one"), identity("two")], &[transcript], &root);
        assert!(matches.assignments.is_empty());
        assert_eq!(matches.ambiguous.len(), 2);
        assert_eq!(matches.warnings.len(), 1);
    }

    /// A fresh Claude launch for an agent that already has an id prepares a NEW
    /// id and writes nothing: the old id stays recorded (and resumable) until
    /// the launch that uses the new one has actually started.
    #[test]
    fn fresh_claude_replacement_retains_old_uuid_until_spawn_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        fs::create_dir(&cwd).unwrap();
        let store = SessionStore::open(&temp.path().join("sessions.sqlite3")).unwrap();
        let old_id = Uuid::new_v4().to_string();
        let session = managed_session("agent-one", "claude", &cwd, &cwd);
        store.create_session(&session).unwrap();
        store
            .set_provider_session_id(&session.id, "claude", &old_id)
            .unwrap();
        let roots = ProviderDataRoots {
            claude_projects: temp.path().join("claude"),
            codex_sessions: temp.path().join("codex"),
        };

        let capture =
            prepare_fresh_capture(&ProviderSessionLaunch::CaptureClaude, &cwd, &roots).unwrap();
        let replacement_id = capture
            .claude_session_id()
            .expect("expected Claude capture")
            .to_string();

        assert_ne!(replacement_id, old_id);
        assert_eq!(
            store.provider_session_id(&session.id, "claude").unwrap(),
            Some(old_id)
        );
    }

    #[test]
    fn claude_artifact_copy_is_atomic_and_no_clobber() {
        let temp = tempfile::tempdir().unwrap();
        let source_dir = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source_dir).unwrap();
        let id = Uuid::new_v4().to_string();
        let transcript = source_dir.join(format!("{id}.jsonl"));
        fs::write(&transcript, b"history\n").unwrap();
        fs::create_dir(source_dir.join(&id)).unwrap();
        fs::write(source_dir.join(&id).join("tool.txt"), b"result").unwrap();

        assert_eq!(copy_claude_artifacts(&transcript, &destination).unwrap(), 2);
        assert_eq!(
            fs::read(destination.join(format!("{id}.jsonl"))).unwrap(),
            b"history\n"
        );
        assert_eq!(
            fs::read(destination.join(&id).join("tool.txt")).unwrap(),
            b"result"
        );
        assert!(transcript.exists());

        fs::write(&transcript, b"changed\n").unwrap();
        assert_eq!(copy_claude_artifacts(&transcript, &destination).unwrap(), 0);
        assert_eq!(
            fs::read(destination.join(format!("{id}.jsonl"))).unwrap(),
            b"history\n"
        );
    }

    #[test]
    fn claude_artifact_copy_refuses_a_symlink_companion() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join(format!("{}.jsonl", Uuid::new_v4()));
        fs::write(&source, b"history\n").unwrap();
        let real_companion = temp.path().join("real-companion");
        fs::create_dir(&real_companion).unwrap();
        symlink(&real_companion, source.with_extension("")).unwrap();

        let error = copy_claude_artifacts(&source, &temp.path().join("destination"))
            .expect_err("symlink companion must fail closed");
        assert!(error.to_string().contains("refusing symlink companion"));
    }

    /// One capture per canonical directory at a time: a second fresh Codex
    /// launch in a directory another capture owns goes uncaptured (it could
    /// not tell the two new rollouts apart), and captures again once the first
    /// released it. The fork waited here instead; see `CodexCapture::begin`.
    #[test]
    fn codex_fresh_capture_serializes_by_canonical_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        let sessions = temp.path().join("sessions");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let coordinator = CodexCaptureCoordinator::default();
        let first = CodexCapture::begin_with(coordinator.clone(), &cwd, &sessions)
            .unwrap()
            .expect("the first capture owns the directory");
        // The same directory through a different spelling is the same key.
        let alias = temp.path().join(".").join("cwd");
        let (tx, rx) = mpsc::channel();
        let second_coordinator = coordinator.clone();
        let second_sessions = sessions.clone();
        std::thread::spawn(move || {
            tx.send(CodexCapture::begin_with(second_coordinator, alias, &second_sessions).unwrap())
                .unwrap();
        });
        let second = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("begin must never wait on another capture");
        assert!(
            second.is_none(),
            "a second capture in the directory is refused"
        );
        first.abort();
        let third = CodexCapture::begin_with(coordinator, &cwd, &sessions)
            .unwrap()
            .expect("released directory captures again");
        third.abort();
    }

    #[test]
    fn claude_project_encoding_matches_wrapper_rules() {
        assert_eq!(
            encode_claude_project_dir(Path::new("/Users/A B/repo_name")),
            "-Users-A-B-repo-name"
        );
    }

    #[test]
    fn claude_bridge_stub_is_not_a_resume_target() {
        let temp = tempfile::tempdir().unwrap();
        let transcript = temp.path().join("session.jsonl");

        fs::write(&transcript, br#"{"type":"bridge-session"}"#).unwrap();
        assert!(!is_resumable_claude_transcript(&transcript));

        fs::write(&transcript, br#"{"type":"mode"}"#).unwrap();
        assert!(is_resumable_claude_transcript(&transcript));
    }

    #[test]
    fn recovery_copies_all_claude_artifacts_and_persists_latest_uuid_once() {
        let temp = tempfile::tempdir().unwrap();
        let historical_root = temp.path().join("worktrees");
        let project_path = temp.path().join("project-checkout");
        let claude_projects = temp.path().join("claude-projects");
        let old_provider_dir = claude_projects.join("old-encoded-dir");
        // A shared agent now runs in the project checkout itself, so only its
        // stored handle can tie it to its old per-agent worktree.
        let destination_cwd = project_path.clone();
        fs::create_dir_all(&old_provider_dir).unwrap();
        fs::create_dir_all(&destination_cwd).unwrap();
        fs::create_dir_all(&project_path).unwrap();
        let old_cwd = historical_root.join("demo/agent-one");
        let older_id = Uuid::new_v4().to_string();
        let newer_id = Uuid::new_v4().to_string();
        for (id, timestamp) in [
            (&older_id, "2026-01-01T00:00:00Z"),
            (&newer_id, "2026-02-01T00:00:00Z"),
        ] {
            let record = serde_json::json!({
                "sessionId": id,
                "cwd": old_cwd,
                "timestamp": timestamp,
            });
            fs::write(
                old_provider_dir.join(format!("{id}.jsonl")),
                format!("{record}\n"),
            )
            .unwrap();
            fs::create_dir(old_provider_dir.join(id)).unwrap();
            fs::write(old_provider_dir.join(id).join("tool.txt"), id).unwrap();
        }

        let mut session = managed_session("session-one", "claude", &project_path, &destination_cwd);
        session.agent_handle = "agent-one".to_string();
        session.shared_workspace = true;
        let project = project("demo", &project_path);
        let store = SessionStore::open(&temp.path().join("sessions.sqlite3")).unwrap();
        store.create_session(&session).unwrap();
        let roots = ProviderDataRoots {
            claude_projects: claude_projects.clone(),
            codex_sessions: temp.path().join("codex-sessions"),
        };
        assert!(recovery_has_candidates(
            std::slice::from_ref(&session),
            std::slice::from_ref(&project),
            &store
        ));

        let report = recover_stranded_histories(
            std::slice::from_ref(&session),
            std::slice::from_ref(&project),
            &historical_root,
            &roots,
            &store,
        )
        .unwrap();
        assert_eq!(report.updates.len(), 1);
        assert_eq!(report.updates[0].provider_session_id, newer_id);
        assert_eq!(report.copied_artifacts, 4);
        let destination = claude_projects.join(encode_claude_project_dir(&destination_cwd));
        assert!(destination.join(format!("{older_id}.jsonl")).is_file());
        assert!(destination.join(&older_id).join("tool.txt").is_file());
        assert!(destination.join(format!("{newer_id}.jsonl")).is_file());
        assert!(old_provider_dir.join(format!("{older_id}.jsonl")).is_file());
        assert_eq!(
            store.provider_session_id(&session.id, "claude").unwrap(),
            Some(newer_id)
        );

        // Recovered once: the agent now has an id, so it is no longer a
        // candidate and a second pass touches nothing.
        assert!(!recovery_has_candidates(
            std::slice::from_ref(&session),
            std::slice::from_ref(&project),
            &store
        ));
        let second =
            recover_stranded_histories(&[session], &[project], &historical_root, &roots, &store)
                .unwrap();
        assert!(second.updates.is_empty());
        assert_eq!(second.copied_artifacts, 0);
    }

    #[test]
    fn codex_capture_identifies_one_new_rollout_for_the_canonical_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        let sessions = temp.path().join("sessions");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(sessions.join("2026/07/14")).unwrap();
        let capture = CodexCapture::begin_with(CodexCaptureCoordinator::default(), &cwd, &sessions)
            .unwrap()
            .unwrap();
        let id = Uuid::new_v4().to_string();
        let record = serde_json::json!({
            "type": "session_meta",
            "payload": {
                "id": id,
                "cwd": fs::canonicalize(&cwd).unwrap(),
                "timestamp": "2026-07-14T12:00:00Z",
            }
        });
        let rollout = sessions
            .join("2026/07/14")
            .join(format!("rollout-test-{id}.jsonl"));
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            fs::write(rollout, format!("{record}\n")).unwrap();
        });

        assert_eq!(
            capture
                .wait_for_id(Some(Duration::from_secs(5)), None)
                .unwrap(),
            Some(id)
        );
        writer.join().unwrap();
        capture.resolve();
    }

    #[test]
    fn codex_capture_stops_when_the_uncaptured_process_exits() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        let sessions = temp.path().join("sessions");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let capture = CodexCapture::begin_with(CodexCaptureCoordinator::default(), &cwd, &sessions)
            .unwrap()
            .unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let process_id = child.id();
        child.wait().unwrap();

        assert_eq!(
            capture
                .wait_for_id(Some(Duration::from_secs(5)), Some(process_id))
                .unwrap(),
            None
        );
        capture.abort();
    }

    #[test]
    fn jcode_session_id_shape_validation() {
        assert!(jcode_session_id_is_valid(
            "session_cactus_1788156095921_18c33bc3e9ed4d80"
        ));
        assert!(jcode_session_id_is_valid("session_guppy2_1_a"));
        // Not UUIDs, wrong prefixes, missing segments, or shell-hostile
        // characters must all be rejected before reaching `--resume`.
        assert!(!jcode_session_id_is_valid(
            "0b8dbb64-92a4-4bc1-a62c-27b6f6e2dc9f"
        ));
        assert!(!jcode_session_id_is_valid("session_cactus_1788156095921"));
        assert!(!jcode_session_id_is_valid("session__1788156095921_18c3"));
        assert!(!jcode_session_id_is_valid("session_cactus_17x8_18c3"));
        assert!(!jcode_session_id_is_valid("session_cactus_1_zz; rm -rf"));
        assert!(!jcode_session_id_is_valid(
            "session_cactus_1_a_extra-segment"
        ));
        assert!(!jcode_session_id_is_valid("rollout_cactus_1_a"));
        assert!(!jcode_session_id_is_valid(""));
    }

    fn write_jcode_metadata(db: &Path, rows: &[(&str, &str, i64)]) {
        let conn = rusqlite::Connection::open(db).unwrap();
        // Only the columns the resolver queries; jcode's real table carries
        // more, which SELECT-by-name is indifferent to.
        conn.execute(
            "CREATE TABLE recent_sessions (
                session_id TEXT, working_dir TEXT, updated_at_ms INTEGER)",
            [],
        )
        .unwrap();
        for (id, dir, updated) in rows {
            conn.execute(
                "INSERT INTO recent_sessions VALUES (?1, ?2, ?3)",
                rusqlite::params![id, dir, updated],
            )
            .unwrap();
        }
    }

    #[test]
    fn resolve_latest_jcode_session_picks_newest_nonempty_for_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().join("wt");
        let other = dir.path().join("elsewhere");
        fs::create_dir_all(&worktree).unwrap();
        let sessions = dir.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let db = dir.path().join("meta.sqlite3");

        let wt = worktree.to_string_lossy().to_string();
        write_jcode_metadata(
            &db,
            &[
                // Newest for the worktree, but a near-empty idle shell.
                ("session_ant_30_aa", &wt, 30),
                // Metadata row whose id shape is wrong: skipped.
                ("not-a-jcode-id", &wt, 25),
                // The one that should win: newest resumable match.
                ("session_bee_20_bb", &wt, 20),
                // Even older resumable match: shadowed by session_bee.
                ("session_cow_10_cc", &wt, 10),
                // Newer than everything but belongs to another directory.
                ("session_owl_99_dd", &other.to_string_lossy(), 99),
            ],
        );
        let big = vec![b'x'; MIN_RESUMABLE_JCODE_SESSION_BYTES as usize];
        fs::write(sessions.join("session_ant_30_aa.json"), b"{}").unwrap();
        fs::write(sessions.join("session_bee_20_bb.json"), &big).unwrap();
        fs::write(sessions.join("session_cow_10_cc.json"), &big).unwrap();
        fs::write(sessions.join("session_owl_99_dd.json"), &big).unwrap();

        assert_eq!(
            resolve_latest_jcode_session(&db, &sessions, &worktree),
            Some("session_bee_20_bb".to_string())
        );
    }

    #[test]
    fn resolve_latest_jcode_session_absent_state_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().join("wt");
        fs::create_dir_all(&worktree).unwrap();
        let sessions = dir.path().join("sessions");

        // No metadata database at all.
        assert_eq!(
            resolve_latest_jcode_session(&dir.path().join("missing.sqlite3"), &sessions, &worktree),
            None
        );

        // Metadata rows exist but every session file is missing or tiny.
        let db = dir.path().join("meta.sqlite3");
        write_jcode_metadata(
            &db,
            &[("session_ant_30_aa", &worktree.to_string_lossy(), 30)],
        );
        assert_eq!(
            resolve_latest_jcode_session(&db, &sessions, &worktree),
            None
        );
    }

    // ── Launch planning (fork src/app/sessions.rs tests) ────────────────

    fn stock(provider: &str) -> ProviderCommandConfig {
        crate::config::provider_config(
            &crate::config::Config::default(),
            &ProviderKind::new(provider),
        )
    }

    fn input<'a>(
        provider: &'a str,
        config: &'a ProviderCommandConfig,
        stored_id: Option<&'a str>,
        shared: bool,
    ) -> ResumeIdInput<'a> {
        ResumeIdInput {
            provider,
            config,
            stored_id,
            shared,
            directory: Path::new("/tmp/shared-project"),
            started: true,
            directory_has_sibling: false,
        }
    }

    fn never_claude(_: &Path, _: &str) -> bool {
        false
    }

    fn no_jcode(_: &Path) -> Option<String> {
        None
    }

    #[test]
    fn codex_targeted_resume_pins_the_current_workspace() {
        let config = stock("codex");
        let session_id = Uuid::new_v4().to_string();
        let cwd = Path::new("/tmp/shared-project");

        let argv = launch_args(
            &config,
            "codex",
            &ProviderSessionLaunch::ResumeId(session_id.clone()),
            false,
            None,
            cwd,
        )
        .unwrap();

        assert_eq!(
            argv,
            vec![
                "resume".to_string(),
                session_id,
                "-C".to_string(),
                cwd.to_string_lossy().into_owned(),
            ]
        );
    }

    #[test]
    fn fresh_claude_launch_appends_preassigned_session_uuid() {
        let config = stock("claude");
        let session_id = Uuid::new_v4().to_string();

        let argv = launch_args(
            &config,
            "claude",
            &ProviderSessionLaunch::CaptureClaude,
            false,
            Some(&session_id),
            Path::new("/tmp/shared-project"),
        )
        .unwrap();

        assert_eq!(
            argv[argv.len() - 2..],
            ["--session-id".to_string(), session_id]
        );
    }

    #[test]
    fn jcode_stored_native_id_passes_the_resume_gate() {
        let config = stock("jcode");
        let native_id = "session_cactus_1788156095921_18c33bc3e9ed4d80";
        assert_eq!(
            plan_resume_id(
                &input("jcode", &config, Some(native_id), false),
                never_claude,
                no_jcode
            ),
            Some(native_id.to_string())
        );

        // An id that is neither a UUID nor jcode-shaped must not reach
        // `--resume`; with no metadata for the directory the launch is fresh.
        assert_eq!(
            plan_resume_id(
                &input("jcode", &config, Some("guppy; rm -rf /"), false),
                never_claude,
                no_jcode
            ),
            None
        );
    }

    /// jcode resolves its own session from its metadata only when the agent is
    /// the sole jcode agent in the directory, and only after it has run there.
    #[test]
    fn jcode_resolves_latest_only_when_alone_and_started() {
        let config = stock("jcode");
        let found = |_: &Path| Some("session_bee_20_bb".to_string());
        let mut alone = input("jcode", &config, None, false);
        assert_eq!(
            plan_resume_id(&alone, never_claude, found),
            Some("session_bee_20_bb".to_string())
        );
        alone.directory_has_sibling = true;
        assert_eq!(plan_resume_id(&alone, never_claude, found), None);
        alone.directory_has_sibling = false;
        alone.started = false;
        assert_eq!(plan_resume_id(&alone, never_claude, found), None);
    }

    #[test]
    fn shared_agents_in_one_cwd_build_distinct_targeted_resume_argv() {
        let cwd = Path::new("/tmp/shared-project");
        let config = stock("codex");
        let first_id = Uuid::new_v4().to_string();
        let second_id = Uuid::new_v4().to_string();

        let first = plan_resume_id(
            &input("codex", &config, Some(&first_id), true),
            never_claude,
            no_jcode,
        )
        .map(ProviderSessionLaunch::ResumeId)
        .unwrap();
        let second = plan_resume_id(
            &input("codex", &config, Some(&second_id), true),
            never_claude,
            no_jcode,
        )
        .map(ProviderSessionLaunch::ResumeId)
        .unwrap();
        let first_argv = launch_args(&config, "codex", &first, false, None, cwd).unwrap();
        let second_argv = launch_args(&config, "codex", &second, false, None, cwd).unwrap();
        assert_ne!(first_argv, second_argv);
        assert_eq!(first_argv.get(1), Some(&first_id));
        assert_eq!(second_argv.get(1), Some(&second_id));
    }

    #[test]
    fn shared_claude_with_missing_transcript_launches_fresh() {
        let config = stock("claude");
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            plan_resume_id(
                &input("claude", &config, Some(&id), true),
                never_claude,
                no_jcode
            ),
            None
        );
        // The same id in an unshared directory is trusted as stored.
        assert_eq!(
            plan_resume_id(
                &input("claude", &config, Some(&id), false),
                never_claude,
                no_jcode
            ),
            Some(id.clone())
        );
        // And a shared one resumes once its transcript is there.
        assert_eq!(
            plan_resume_id(
                &input("claude", &config, Some(&id), true),
                |_, _| true,
                no_jcode
            ),
            Some(id)
        );
    }

    /// A provider with no `resume_by_id_args` never resumes by id, whatever is
    /// stored, so the stored id cannot reach a CLI that would misread it.
    #[test]
    fn provider_without_resume_by_id_never_plans_one() {
        let config = stock("opencode");
        let id = Uuid::new_v4().to_string();
        assert_eq!(
            plan_resume_id(
                &input("opencode", &config, Some(&id), false),
                |_, _| true,
                no_jcode
            ),
            None
        );
        assert_eq!(
            fresh_capture_for("opencode", &config),
            ProviderSessionLaunch::Plain
        );
        assert_eq!(
            fresh_capture_for("claude", &stock("claude")),
            ProviderSessionLaunch::CaptureClaude
        );
        assert_eq!(
            fresh_capture_for("codex", &stock("codex")),
            ProviderSessionLaunch::CaptureCodex
        );
    }
}
