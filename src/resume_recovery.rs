//! Provider-session capture and one-time recovery of legacy worktree histories.

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

use crate::model::{AgentSession, Project, normalize_agent_handle};
use crate::storage::SessionStore;

const MAX_TRANSCRIPT_FILES: usize = 100_000;
const MAX_JSON_LINE_BYTES: usize = 16 * 1024 * 1024;
const CODEX_PROCESS_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub(crate) struct ProviderDataRoots {
    pub claude_projects: PathBuf,
    pub codex_sessions: PathBuf,
}

impl ProviderDataRoots {
    pub(crate) fn from_home() -> Result<Self> {
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
pub(crate) fn jcode_session_id_is_valid(id: &str) -> bool {
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
pub(crate) fn resolve_latest_jcode_session(
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
pub(crate) fn resolve_latest_jcode_session_from_home(worktree: &Path) -> Option<String> {
    let root = home::home_dir()?.join(".jcode");
    resolve_latest_jcode_session(
        &root.join("session-metadata-v1.sqlite3"),
        &root.join("sessions"),
        worktree,
    )
}

pub(crate) enum FreshCapture {
    None,
    Claude {
        session_id: String,
        persist_after_spawn: bool,
    },
    Codex(CodexCapture),
}

impl FreshCapture {
    pub(crate) fn claude_session_id(&self) -> Option<&str> {
        match self {
            Self::Claude { session_id, .. } => Some(session_id),
            Self::None | Self::Codex(_) => None,
        }
    }

    pub(crate) fn abort(self) {
        if let Self::Codex(capture) = self {
            capture.abort();
        }
    }
}

pub(crate) fn prepare_fresh_capture_from_home(
    session: &mut AgentSession,
    store: &SessionStore,
) -> Result<FreshCapture> {
    if !matches!(session.provider.as_str(), "claude" | "codex") {
        return Ok(FreshCapture::None);
    }
    prepare_fresh_capture(session, store, &ProviderDataRoots::from_home()?)
}

pub(crate) fn prepare_fresh_capture(
    session: &mut AgentSession,
    store: &SessionStore,
    roots: &ProviderDataRoots,
) -> Result<FreshCapture> {
    match session.provider.as_str() {
        "claude" => {
            let session_id = Uuid::new_v4().to_string();
            let persist_after_spawn = session.provider_session_ids.contains_key("claude");
            if !persist_after_spawn {
                store.set_provider_session_id(&session.id, "claude", &session_id)?;
                session
                    .provider_session_ids
                    .insert("claude".to_string(), session_id.clone());
            }
            Ok(FreshCapture::Claude {
                session_id,
                persist_after_spawn,
            })
        }
        "codex" => CodexCapture::begin(&session.worktree_path, &roots.codex_sessions)
            .map(FreshCapture::Codex),
        _ => Ok(FreshCapture::None),
    }
}

#[derive(Clone, Default)]
pub(crate) struct CodexCaptureCoordinator {
    inner: Arc<(Mutex<HashMap<PathBuf, CaptureState>>, Condvar)>,
}

#[derive(Clone)]
enum CaptureState {
    Capturing,
    Blocked(String),
}

pub(crate) struct CodexCapture {
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
    pub(crate) fn begin(cwd: impl AsRef<Path>, sessions_root: &Path) -> Result<Self> {
        Self::begin_with(capture_coordinator().clone(), cwd, sessions_root)
    }

    fn begin_with(
        coordinator: CodexCaptureCoordinator,
        cwd: impl AsRef<Path>,
        sessions_root: &Path,
    ) -> Result<Self> {
        let cwd = fs::canonicalize(cwd.as_ref()).with_context(|| {
            format!(
                "failed to canonicalize Codex launch cwd {}",
                cwd.as_ref().display()
            )
        })?;
        let (state_lock, wake) = &*coordinator.inner;
        let mut states = state_lock.lock().expect("Codex capture mutex poisoned");
        loop {
            match states.get(&cwd) {
                None => {
                    states.insert(cwd.clone(), CaptureState::Capturing);
                    break;
                }
                Some(CaptureState::Capturing) => {
                    states = wake.wait(states).expect("Codex capture mutex poisoned");
                }
                Some(CaptureState::Blocked(reason)) => {
                    bail!("Codex capture is blocked for this workspace: {reason}");
                }
            }
        }
        drop(states);

        match scan_codex_rollouts(sessions_root) {
            Ok(known) => Ok(Self {
                coordinator,
                cwd,
                sessions_root: sessions_root.to_path_buf(),
                known_ids: known.into_iter().map(|rollout| rollout.id).collect(),
                active: true,
            }),
            Err(err) => {
                release_capture(&coordinator, &cwd);
                Err(err)
            }
        }
    }

    pub(crate) fn wait_for_id(
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

    pub(crate) fn resolve(mut self) {
        release_capture(&self.coordinator, &self.cwd);
        self.active = false;
    }

    pub(crate) fn abort(mut self) {
        release_capture(&self.coordinator, &self.cwd);
        self.active = false;
    }

    pub(crate) fn block(mut self, reason: &str) {
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
pub(crate) struct ProviderSessionUpdate {
    pub session_id: String,
    pub provider: String,
    pub provider_session_id: String,
}

#[derive(Default, Debug)]
pub(crate) struct RecoveryReport {
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

pub(crate) fn recover_stranded_histories(
    sessions: &[AgentSession],
    projects: &[Project],
    worktrees_root: &Path,
    roots: &ProviderDataRoots,
    store: &SessionStore,
) -> Result<RecoveryReport> {
    let identities = recovery_identities(sessions, projects);
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
                            crate::sanitize::for_terminal(&identity.session_id),
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
                crate::sanitize::for_terminal(&identity.provider),
                crate::sanitize::for_terminal(&identity.session_id),
            )),
        }
    }
    Ok(report)
}

fn recovery_identities(sessions: &[AgentSession], projects: &[Project]) -> Vec<RecoveryIdentity> {
    let mut identities = Vec::new();
    for session in sessions {
        let Some(project) = projects
            .iter()
            .find(|project| project.id == session.project_id)
        else {
            continue;
        };
        let Some(session_project_path) = session.project_path.as_deref() else {
            continue;
        };
        if !paths_equivalent(Path::new(session_project_path), Path::new(&project.path)) {
            continue;
        }
        for provider in &session.started_providers {
            if !matches!(provider.as_str(), "claude" | "codex")
                || session.provider_session_ids.contains_key(provider)
            {
                continue;
            }
            identities.push(RecoveryIdentity {
                session_id: session.id.clone(),
                provider: provider.clone(),
                agent_handle: session.agent_handle.clone(),
                project_name: project.name.clone(),
                destination_cwd: PathBuf::from(&session.worktree_path),
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
                    .map(|identity| crate::sanitize::for_terminal(&identity.session_id))
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
                    crate::sanitize::for_terminal(&project_name),
                    crate::sanitize::for_terminal(&handle),
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

pub(crate) fn encode_claude_project_dir(path: &Path) -> String {
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

pub(crate) fn claude_resume_target_exists(cwd: &Path, session_id: &str) -> bool {
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

    fn capture_session(id: &str, provider: &str, cwd: &Path) -> AgentSession {
        let now = chrono::Utc::now();
        AgentSession {
            id: id.to_string(),
            project_id: "project-one".to_string(),
            project_path: Some(cwd.to_string_lossy().to_string()),
            provider: crate::model::ProviderKind::new(provider),
            source_branch: "main".to_string(),
            branch_name: id.to_string(),
            worktree_path: cwd.to_string_lossy().to_string(),
            agent_handle: id.to_string(),
            shared_workspace: true,
            deleted_at: None,
            title: None,
            started_providers: vec![provider.to_string()],
            provider_session_ids: Default::default(),
            state: crate::model::SessionState::Created { created_at: now },
            settings: crate::model::SessionSettings::default(),
            created_at: now,
            updated_at: now,
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

    #[test]
    fn fresh_claude_replacement_retains_old_uuid_until_spawn_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        fs::create_dir(&cwd).unwrap();
        let store = SessionStore::open(&temp.path().join("sessions.sqlite3")).unwrap();
        let old_id = Uuid::new_v4().to_string();
        let mut session = capture_session("agent-one", "claude", &cwd);
        session
            .provider_session_ids
            .insert("claude".to_string(), old_id.clone());
        store.upsert_session(&session).unwrap();
        let roots = ProviderDataRoots {
            claude_projects: temp.path().join("claude"),
            codex_sessions: temp.path().join("codex"),
        };

        let capture = prepare_fresh_capture(&mut session, &store, &roots).unwrap();
        let FreshCapture::Claude {
            session_id: replacement_id,
            persist_after_spawn,
        } = capture
        else {
            panic!("expected Claude capture");
        };

        assert!(persist_after_spawn);
        assert_ne!(replacement_id, old_id);
        assert_eq!(
            store.load_sessions().unwrap()[0]
                .provider_session_ids
                .get("claude"),
            Some(&old_id)
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

    #[test]
    fn codex_fresh_capture_serializes_by_canonical_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("cwd");
        let sessions = temp.path().join("sessions");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&sessions).unwrap();
        let coordinator = CodexCaptureCoordinator::default();
        let first = CodexCapture::begin_with(coordinator.clone(), &cwd, &sessions).unwrap();
        let (tx, rx) = mpsc::channel();
        let second_cwd = cwd.clone();
        let second_sessions = sessions.clone();
        std::thread::spawn(move || {
            tx.send(CodexCapture::begin_with(
                coordinator,
                second_cwd,
                &second_sessions,
            ))
            .unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        first.abort();
        let second = rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
        second.abort();
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
        let destination_cwd = project_path.clone();
        fs::create_dir_all(&old_provider_dir).unwrap();
        fs::create_dir_all(&destination_cwd).unwrap();
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

        let now = chrono::Utc::now();
        let session = AgentSession {
            id: "session-one".to_string(),
            project_id: "project-one".to_string(),
            project_path: Some(project_path.to_string_lossy().to_string()),
            provider: crate::model::ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "agent-one".to_string(),
            worktree_path: destination_cwd.to_string_lossy().to_string(),
            agent_handle: "agent-one".to_string(),
            shared_workspace: true,
            deleted_at: None,
            title: None,
            started_providers: vec!["claude".to_string()],
            provider_session_ids: Default::default(),
            state: crate::model::SessionState::Created { created_at: now },
            settings: crate::model::SessionSettings::default(),
            created_at: now,
            updated_at: now,
        };
        let project = Project {
            id: "project-one".to_string(),
            name: "demo".to_string(),
            path: project_path.to_string_lossy().to_string(),
            default_provider: crate::model::ProviderKind::new("claude"),
            current_branch: "main".to_string(),
            path_missing: false,
            meta_loaded: true,
        };
        let store = SessionStore::open(&temp.path().join("sessions.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        let roots = ProviderDataRoots {
            claude_projects: claude_projects.clone(),
            codex_sessions: temp.path().join("codex-sessions"),
        };

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
            store.load_sessions().unwrap()[0]
                .provider_session_ids
                .get("claude"),
            Some(&newer_id)
        );

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
        let capture =
            CodexCapture::begin_with(CodexCaptureCoordinator::default(), &cwd, &sessions).unwrap();
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
                .wait_for_id(Some(Duration::from_secs(2)), None)
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
        let capture =
            CodexCapture::begin_with(CodexCaptureCoordinator::default(), &cwd, &sessions).unwrap();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let process_id = child.id();
        child.wait().unwrap();

        assert_eq!(
            capture
                .wait_for_id(Some(Duration::from_secs(2)), Some(process_id))
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
}
