//! Peer routing, shared-workspace safeguards, and globally locked AMQ lifecycle.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use rustix::fs::{FlockOperation, flock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::config::DuxPaths;
use crate::model::AgentSession;
use crate::pty::PerSessionEnv;
use crate::storage::SessionStore;

const DEFAULT_CLAUDE_PEERS_PORT: u16 = 7899;
const OWNER_MARKER: &str = ".dux-amq-source";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportPreference {
    Auto,
    Amq,
    ClaudePeers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChosenTransport {
    Amq,
    ClaudePeers,
}

#[derive(Clone, Debug)]
struct PeerTarget {
    handle: String,
    session: Option<AgentSession>,
}

#[derive(Clone, Debug)]
struct SenderContext {
    handle: String,
    session: Option<AgentSession>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AmqSyncReport {
    pub root: Option<PathBuf>,
    pub configured_agents_added: usize,
    pub stale_config_agents_removed: usize,
    pub ownership_markers_created: usize,
    pub handles_deconflicted: usize,
    pub skipped: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ClaudePeer {
    id: String,
    cwd: String,
}

pub fn run_peer(args: &[String], paths: &DuxPaths) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "send" => run_peer_send(&args[1..], paths),
        "list" => run_peer_list(&args[1..], paths),
        "sync-amq" | "sync" => run_peer_sync_amq(&args[1..], paths),
        "" | "--help" | "-h" => {
            print_peer_help();
            Ok(())
        }
        other => bail!("unknown peer subcommand: {other}\nRun `dux peer --help` for usage."),
    }
}

pub fn append_session_env(env: &mut PerSessionEnv, session: &AgentSession, store_id: &str) {
    env.vars
        .push(("DUX_SESSION_ID".to_string(), session.id.clone()));
    env.vars
        .push(("DUX_STORE_ID".to_string(), store_id.to_string()));
    env.vars.push((
        "DUX_PROVIDER".to_string(),
        session.provider.as_str().to_string(),
    ));
    env.vars.push((
        "DUX_AMQ_HANDLE".to_string(),
        session.agent_handle().to_string(),
    ));
}

pub fn sync_amq_agents(paths: &DuxPaths) -> Result<AmqSyncReport> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(AmqSyncReport {
            skipped: true,
            ..AmqSyncReport::default()
        });
    };
    let store_id = crate::storage::load_or_create_store_id(&paths.root)?;
    if !paths.sessions_db_path.exists() {
        return reconcile_amq_root(&root, &store_id, None, &mut []);
    }
    let store = SessionStore::open(&paths.sessions_db_path)
        .with_context(|| format!("failed to open {}", paths.sessions_db_path.display()))?;
    let mut sessions = store.load_sessions_including_deleted()?;
    reconcile_amq_root(&root, &store_id, Some(&store), &mut sessions)
}

fn run_peer_send(args: &[String], paths: &DuxPaths) -> Result<()> {
    let parsed = parse_send_args(args)?;
    let _ = sync_amq_agents(paths)?;
    let sessions = load_sessions_if_present(paths)?;

    let sender = infer_sender(parsed.from.as_deref(), &sessions)?;
    let target = resolve_target(&parsed.target, &sessions)?;
    let transport = choose_transport(parsed.transport, &sender, &target)?;

    match transport {
        ChosenTransport::ClaudePeers => {
            let target_session = target
                .session
                .as_ref()
                .ok_or_else(|| anyhow!("Claude Peers requires a known Claude target session"))?;
            let peers = claude_peers_list().context(
                "Claude-targeted routes require Claude Peers, but the broker is unavailable. \
                 Restart Claude agents after installing claude-peers, or pass --transport amq \
                 explicitly for a manual override",
            )?;
            let to_id = claude_peer_id_for_session(target_session, &peers).ok_or_else(|| {
                anyhow!(
                    "Claude Peers is not registered for target {}",
                    amq_handle_for_session(target_session)
                )
            })?;
            let (from_id, message) = claude_peers_sender(&sender, &peers, &parsed.message);
            claude_peers_send(&from_id, &to_id, &message)?;
            println!(
                "sent via claude-peers: {} -> {}",
                sender.handle, target.handle
            );
        }
        ChosenTransport::Amq => {
            let root = require_amq_root(paths)?;
            amq_send(&root, &sender.handle, &target.handle, &parsed.message)?;
            println!("sent via amq: {} -> {}", sender.handle, target.handle);
        }
    }

    Ok(())
}

fn run_peer_list(args: &[String], paths: &DuxPaths) -> Result<()> {
    reject_unknown_peer_flags(args)?;
    let report = sync_amq_agents(paths)?;
    let sessions = load_sessions_if_present(paths)?;

    println!("Dux peers:");
    if sessions.is_empty() {
        println!("  (no persisted sessions)");
    } else {
        for session in &sessions {
            println!(
                "  {:<32} {:<8} {}",
                amq_handle_for_session(session),
                session.provider.as_str(),
                session.worktree_path
            );
        }
    }

    if report.skipped {
        println!("AMQ registry: skipped (no configured AMQ root found)");
    } else if let Some(root) = report.root {
        println!(
            "AMQ registry: {} (added {}, removed stale {}, deconflicted {})",
            root.display(),
            report.configured_agents_added,
            report.stale_config_agents_removed,
            report.handles_deconflicted
        );
    }

    match claude_peers_list() {
        Ok(peers) => println!("Claude Peers broker: {} peer(s) registered", peers.len()),
        Err(_) => println!("Claude Peers broker: unavailable"),
    }

    Ok(())
}

fn run_peer_sync_amq(args: &[String], paths: &DuxPaths) -> Result<()> {
    reject_unknown_peer_flags(args)?;
    let report = sync_amq_agents(paths)?;
    if report.skipped {
        println!("AMQ sync skipped: no configured AMQ root found");
        return Ok(());
    }
    let root = report.root.as_ref().expect("non-skipped report has root");
    println!("AMQ sync: {}", root.display());
    println!(
        "  configured agents added: {}",
        report.configured_agents_added
    );
    println!(
        "  stale config agents removed: {}",
        report.stale_config_agents_removed
    );
    println!(
        "  ownership markers created: {}",
        report.ownership_markers_created
    );
    println!("  handles deconflicted: {}", report.handles_deconflicted);
    Ok(())
}

fn print_peer_help() {
    println!(
        "\
dux peer - route messages between Dux agent sessions

Subcommands:
  dux peer send [--from <handle>] [--transport auto|amq|claude-peers] <target> <message...>
                       Send through the Dux router. Auto uses AMQ when either
                       endpoint is shared; otherwise Claude targets use Peers.
  dux peer list        List Dux sessions and transport health.
  dux peer sync-amq    Reconcile AMQ's agent registry from sessions.sqlite3.

Agents should use this command instead of calling amq or Claude Peers directly."
    );
}

struct SendArgs {
    from: Option<String>,
    transport: TransportPreference,
    target: String,
    message: String,
}

fn parse_send_args(args: &[String]) -> Result<SendArgs> {
    let mut from = None;
    let mut transport = TransportPreference::Auto;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if !positional.is_empty() {
            positional.extend(args[i..].iter().cloned());
            break;
        }
        match args[i].as_str() {
            "--from" => {
                i += 1;
                let Some(value) = args.get(i) else {
                    bail!("--from requires a value");
                };
                from = Some(value.clone());
            }
            "--transport" => {
                i += 1;
                let Some(value) = args.get(i) else {
                    bail!("--transport requires auto, amq, or claude-peers");
                };
                transport = match value.as_str() {
                    "auto" => TransportPreference::Auto,
                    "amq" => TransportPreference::Amq,
                    "claude-peers" => TransportPreference::ClaudePeers,
                    other => bail!("unknown transport: {other}"),
                };
            }
            "--" => {
                positional.extend(args[i + 1..].iter().cloned());
                break;
            }
            "-h" | "--help" => {
                print_peer_help();
                std::process::exit(0);
            }
            arg if arg.starts_with('-') => bail!("unknown flag: {arg}"),
            _ => positional.push(args[i].clone()),
        }
        i += 1;
    }

    if positional.len() < 2 {
        bail!("usage: dux peer send <target> <message...>");
    }
    Ok(SendArgs {
        from,
        transport,
        target: positional[0].clone(),
        message: positional[1..].join(" "),
    })
}

fn reject_unknown_peer_flags(args: &[String]) -> Result<()> {
    for arg in args {
        if arg.starts_with('-') {
            bail!("unknown flag: {arg}");
        }
    }
    Ok(())
}

fn load_sessions_if_present(paths: &DuxPaths) -> Result<Vec<AgentSession>> {
    if !paths.sessions_db_path.exists() {
        return Ok(Vec::new());
    }
    let sessions = SessionStore::open(&paths.sessions_db_path)
        .with_context(|| format!("failed to open {}", paths.sessions_db_path.display()))?
        .load_sessions()
        .context("failed to load Dux sessions")?;
    Ok(sessions.into_iter().filter(is_routable_session).collect())
}

fn is_routable_session(session: &AgentSession) -> bool {
    !session.state.is_exited() && Path::new(&session.worktree_path).exists()
}

fn infer_sender(from: Option<&str>, sessions: &[AgentSession]) -> Result<SenderContext> {
    if let Some(from) = from {
        let handle = sanitise_handle(from);
        if handle.is_empty() {
            bail!("--from normalizes to an empty AMQ handle");
        }
        if let Some(session) = sessions
            .iter()
            .find(|session| session.agent_handle() == handle)
            .cloned()
        {
            return Ok(SenderContext {
                handle,
                session: Some(session),
            });
        }
        let session = sessions
            .iter()
            .find(|session| {
                let aliases = session_aliases(session);
                aliases.contains(from) || aliases.contains(&handle)
            })
            .cloned();
        return Ok(SenderContext { handle, session });
    }

    if let Ok(id) = env::var("DUX_SESSION_ID")
        && let Some(session) = sessions.iter().find(|session| session.id == id)
    {
        return Ok(SenderContext {
            handle: amq_handle_for_session(session),
            session: Some(session.clone()),
        });
    }

    for var in ["DUX_AMQ_HANDLE", "AM_ME"] {
        if let Ok(value) = env::var(var) {
            let handle = sanitise_handle(&value);
            if !handle.is_empty() {
                let session = sessions
                    .iter()
                    .find(|session| amq_handle_for_session(session) == handle)
                    .cloned();
                return Ok(SenderContext { handle, session });
            }
        }
    }

    if let Ok(cwd) = env::current_dir()
        && let Some(session) = session_for_cwd(&cwd, sessions)?
    {
        return Ok(SenderContext {
            handle: amq_handle_for_session(session),
            session: Some(session.clone()),
        });
    }

    Ok(SenderContext {
        handle: "dux-router".to_string(),
        session: None,
    })
}

fn resolve_target(target: &str, sessions: &[AgentSession]) -> Result<PeerTarget> {
    let sanitized = sanitise_handle(target);
    let mut matches = Vec::new();
    for session in sessions {
        let aliases = session_aliases(session);
        if aliases.contains(target) || (!sanitized.is_empty() && aliases.contains(&sanitized)) {
            matches.push(session.clone());
        }
    }

    if matches.len() > 1 {
        let names = matches
            .iter()
            .map(|session| format!("{} ({})", amq_handle_for_session(session), session.id))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("ambiguous peer target {target:?}; matches: {names}");
    }

    if let Some(session) = matches.pop() {
        return Ok(PeerTarget {
            handle: amq_handle_for_session(&session),
            session: Some(session),
        });
    }

    if sanitized.is_empty() {
        bail!("target normalizes to an empty AMQ handle");
    }
    bail!(
        "agent {target:?} does not exist or is not reachable; run `dux peer list` and retry with a listed handle"
    )
}

fn choose_transport(
    preference: TransportPreference,
    sender: &SenderContext,
    target: &PeerTarget,
) -> Result<ChosenTransport> {
    let shared_endpoint = sender
        .session
        .as_ref()
        .is_some_and(AgentSession::shared_workspace)
        || target
            .session
            .as_ref()
            .is_some_and(AgentSession::shared_workspace);
    if shared_endpoint {
        if preference == TransportPreference::ClaudePeers {
            bail!(
                "Claude Peers cannot route shared-workspace sessions; use --transport amq or omit --transport"
            );
        }
        return Ok(ChosenTransport::Amq);
    }

    match preference {
        TransportPreference::Amq => Ok(ChosenTransport::Amq),
        TransportPreference::ClaudePeers => {
            ensure_claude_peers_target(target)?;
            Ok(ChosenTransport::ClaudePeers)
        }
        TransportPreference::Auto => {
            if is_claude_session(target.session.as_ref()) {
                Ok(ChosenTransport::ClaudePeers)
            } else {
                Ok(ChosenTransport::Amq)
            }
        }
    }
}

fn ensure_claude_peers_target(target: &PeerTarget) -> Result<()> {
    if !is_claude_session(target.session.as_ref()) {
        let provider = target
            .session
            .as_ref()
            .map(|s| s.provider.as_str())
            .unwrap_or("unknown");
        bail!("Claude Peers transport cannot target provider {provider}; use AMQ");
    }
    Ok(())
}

fn is_claude_session(session: Option<&AgentSession>) -> bool {
    session
        .map(|session| session.provider.as_str() == "claude")
        .unwrap_or(false)
}

fn amq_send(root: &Path, from: &str, target: &str, message: &str) -> Result<()> {
    let output = Command::new("amq")
        .arg("send")
        .arg("--to")
        .arg(target)
        .arg("--body")
        .arg(message)
        .env("AM_ROOT", root)
        .env("AM_ME", from)
        .output()
        .context("failed to execute amq")?;
    if !output.status.success() {
        bail!(
            "amq send failed: {}",
            crate::sanitize::utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn claude_peers_send(from_id: &str, to_id: &str, message: &str) -> Result<()> {
    let body = json!({
        "from_id": from_id,
        "to_id": to_id,
        "text": message,
    });
    let value = claude_peers_post("/send-message", &body)?;
    if value.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return Ok(());
    }
    let error = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unknown broker error");
    bail!("Claude Peers send failed: {error}");
}

fn claude_peers_list() -> Result<Vec<ClaudePeer>> {
    let body = json!({
        "scope": "machine",
        "cwd": "/",
        "git_root": Value::Null,
    });
    let value = claude_peers_post("/list-peers", &body)?;
    serde_json::from_value(value).context("Claude Peers broker returned malformed peer list")
}

fn claude_peers_sender(
    sender: &SenderContext,
    peers: &[ClaudePeer],
    message: &str,
) -> (String, String) {
    if let Some(session) = sender.session.as_ref()
        && is_claude_session(Some(session))
        && let Some(id) = claude_peer_id_for_session(session, peers)
    {
        return (id, message.to_string());
    }

    let body = format!(
        "DUX peer {handle} sent this via Claude Peers.\n\
         Reply with `dux peer send {handle} \"...\"`; this sender is not a Claude Peers peer.\n\n\
         {message}",
        handle = sender.handle,
        message = message,
    );
    (sender.handle.clone(), body)
}

fn claude_peers_post(path: &str, body: &Value) -> Result<Value> {
    let port = env::var("CLAUDE_PEERS_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(DEFAULT_CLAUDE_PEERS_PORT);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(750))
        .with_context(|| format!("Claude Peers broker unavailable on {addr}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    let body = serde_json::to_string(body)?;
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (headers, response_body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed response from Claude Peers broker"))?;
    let status_line = headers.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(0);
    if !(200..300).contains(&status) {
        bail!(
            "Claude Peers broker returned HTTP {status}: {}",
            crate::sanitize::truncate(response_body, 300)
        );
    }
    serde_json::from_str(response_body).context("Claude Peers broker returned non-JSON response")
}

fn claude_peer_id_for_session(session: &AgentSession, peers: &[ClaudePeer]) -> Option<String> {
    let session_path = canonical_or_raw(Path::new(&session.worktree_path));
    peers
        .iter()
        .find(|peer| canonical_or_raw(Path::new(&peer.cwd)) == session_path)
        .map(|peer| peer.id.clone())
}

fn session_for_cwd<'a>(
    cwd: &Path,
    sessions: &'a [AgentSession],
) -> Result<Option<&'a AgentSession>> {
    let cwd = canonical_or_raw(cwd);
    let mut nearest = None;
    let mut ambiguous = false;
    for session in sessions {
        let path = canonical_or_raw(Path::new(&session.worktree_path));
        if cwd == path || cwd.starts_with(&path) {
            let depth = path.components().count();
            match nearest {
                Some((nearest_depth, _)) if depth < nearest_depth => {}
                Some((nearest_depth, _)) if depth == nearest_depth => ambiguous = true,
                _ => {
                    nearest = Some((depth, session));
                    ambiguous = false;
                }
            }
        }
    }
    if ambiguous {
        bail!(
            "cannot infer sender because multiple Dux sessions share this workspace; pass --from <agent_handle>"
        );
    }
    Ok(nearest.map(|(_, session)| session))
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn session_aliases(session: &AgentSession) -> HashSet<String> {
    let mut aliases = HashSet::new();
    aliases.insert(session.id.clone());
    aliases.insert(session.branch_name.clone());
    aliases.insert(amq_handle_for_session(session));
    if let Some(title) = &session.title {
        aliases.insert(title.clone());
    }
    if let Some(name) = Path::new(&session.worktree_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        aliases.insert(name.to_string());
        let sanitized = sanitise_handle(name);
        if !sanitized.is_empty() {
            aliases.insert(sanitized);
        }
    }
    aliases
}

pub(crate) fn amq_handle_for_session(session: &AgentSession) -> String {
    session.agent_handle().to_string()
}

fn sanitise_handle(name: &str) -> String {
    crate::sanitize::amq_handle(name)
}

fn optional_amq_root(paths: &DuxPaths) -> Option<PathBuf> {
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

pub(crate) fn amq_cleanup_requires_worker(
    paths: &DuxPaths,
    store_id: &str,
    session: &AgentSession,
) -> bool {
    let Some(root) = optional_amq_root(paths) else {
        return false;
    };
    match marker_state(&root, session.agent_handle()) {
        Ok(MarkerState::Owner(owner)) => {
            owner.store_id == store_id && owner.session_id == session.id
        }
        Ok(MarkerState::Free) => false,
        Ok(MarkerState::Legacy(_) | MarkerState::Foreign) | Err(_) => true,
    }
}

fn require_amq_root(paths: &DuxPaths) -> Result<PathBuf> {
    optional_amq_root(paths).ok_or_else(|| {
        anyhow!("AMQ root is not configured; set AMQ_GLOBAL_ROOT or install dux-amq")
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct OwnerMarker {
    store_id: String,
    session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    wake_pid: Option<u32>,
}

enum MarkerState {
    Free,
    Owner(OwnerMarker),
    Legacy(PathBuf),
    Foreign,
}

struct AmqRegistryLock {
    file: File,
}

impl AmqRegistryLock {
    fn acquire(root: &Path) -> Result<Self> {
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

/// Persist a new row and reserve its global AMQ identity before provider
/// launch. A failed reservation can leave the row for retry, but never an
/// owner marker without its row.
pub(crate) fn reserve_and_persist_session(
    paths: &DuxPaths,
    store_id: &str,
    store: &SessionStore,
    session: &mut AgentSession,
) -> Result<()> {
    let Some(root) = optional_amq_root(paths) else {
        store.assign_unique_agent_handle(session)?;
        return store.upsert_session(session);
    };
    reserve_and_persist_session_at_root(&root, store_id, store, session)
}

fn reserve_and_persist_session_at_root(
    root: &Path,
    store_id: &str,
    store: &SessionStore,
    session: &mut AgentSession,
) -> Result<()> {
    let _lock = AmqRegistryLock::acquire(root)?;
    fs::create_dir_all(root.join("agents"))
        .with_context(|| format!("failed to create {}", root.join("agents").display()))?;
    let owner = OwnerMarker {
        store_id: store_id.to_string(),
        session_id: session.id.clone(),
        wake_pid: None,
    };
    let used = store
        .load_sessions_including_deleted()?
        .into_iter()
        .map(|row| row.agent_handle().to_string())
        .collect::<HashSet<_>>();
    let base = crate::model::normalize_agent_handle(session.agent_handle());
    if base.is_empty() {
        bail!("new session has an empty agent handle");
    }
    let handle = if !used.contains(&base) && marker_is_claimable(root, &base, &owner)? {
        base
    } else {
        next_global_handle(root, &base, &used, &owner)?
    };
    session.agent_handle = handle.clone();
    store.upsert_session(session)?;
    ensure_owner_marker(root, &handle, &owner)?;
    register_config_handle(root, &handle)?;
    Ok(())
}

fn reconcile_amq_root(
    root: &Path,
    store_id: &str,
    store: Option<&SessionStore>,
    sessions: &mut [AgentSession],
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
        let owner = OwnerMarker {
            store_id: store_id.to_string(),
            session_id: sessions[index].id.clone(),
            wake_pid: None,
        };
        let current = sessions[index].agent_handle().to_string();
        let claimable = match marker_state(root, &current)? {
            MarkerState::Free => true,
            MarkerState::Owner(existing) => same_owner(&existing, &owner),
            MarkerState::Legacy(path) => {
                let matches = sessions
                    .iter()
                    .filter(|candidate| {
                        paths_equivalent(Path::new(&candidate.worktree_path), &path)
                    })
                    .count();
                matches == 1 && paths_equivalent(Path::new(&sessions[index].worktree_path), &path)
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
        .filter(|session| session.deleted_at.is_none())
        .map(|session| session.agent_handle().to_string())
        .collect::<BTreeSet<_>>();
    let sessions_by_id = sessions
        .iter()
        .map(|session| (session.id.as_str(), session))
        .collect::<HashMap<_, _>>();
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
    agents.retain(|handle| match marker_state(root, handle) {
        Ok(MarkerState::Owner(owner)) if owner.store_id == store_id => {
            !inventory_complete
                || sessions_by_id
                    .get(owner.session_id.as_str())
                    .is_some_and(|session| {
                        session.deleted_at.is_none() && session.agent_handle() == handle
                    })
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

fn marker_state(root: &Path, handle: &str) -> Result<MarkerState> {
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

fn ensure_owner_marker(root: &Path, handle: &str, owner: &OwnerMarker) -> Result<()> {
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
    for suffix_number in 2u64.. {
        let suffix = format!("-{suffix_number}");
        let prefix_len = crate::model::AGENT_HANDLE_MAX_LEN.saturating_sub(suffix.len());
        let prefix = base.chars().take(prefix_len).collect::<String>();
        let candidate = format!("{prefix}{suffix}");
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

fn register_config_handle(root: &Path, handle: &str) -> Result<()> {
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

/// Stop delivery and remove a session from the live registry while retaining
/// its inbox and exact owner marker as an ordinary-delete tombstone.
pub(crate) fn tombstone_amq_session(
    paths: &DuxPaths,
    store_id: &str,
    session: &AgentSession,
) -> Result<()> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(());
    };
    if let Err(err) = tombstone_amq_session_at_root(&root, store_id, session) {
        tracing::warn!(
            target: "dux::peer",
            session_id = %crate::sanitize::for_terminal(&session.id),
            agent_handle = %crate::sanitize::for_terminal(session.agent_handle()),
            error = %crate::sanitize::for_terminal(&format!("{err:#}")),
            "AMQ cleanup failed during session deletion; continuing with the local tombstone"
        );
    }
    Ok(())
}

fn tombstone_amq_session_at_root(
    root: &Path,
    store_id: &str,
    session: &AgentSession,
) -> Result<()> {
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
                tracing::warn!(
                    target: "dux::peer",
                    session_id = %crate::sanitize::for_terminal(&session.id),
                    agent_handle = %crate::sanitize::for_terminal(session.agent_handle()),
                    error = %crate::sanitize::for_terminal(&format!("{err:#}")),
                    "could not verify the AMQ owner marker during deletion; leaving it untouched"
                );
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
/// a global handle. Phase 5 hard purge is the first production caller.
pub fn free_amq_handle(paths: &DuxPaths, store_id: &str, session: &AgentSession) -> Result<()> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(());
    };
    if !root.exists() {
        return Ok(());
    }
    free_amq_handle_at_root(&root, store_id, session)
}

/// Read-only ownership classification used by reset to inventory every handle
/// before freeing any of them. Foreign and legacy registrations are preserved.
pub fn amq_handle_is_exact_owner(
    paths: &DuxPaths,
    store_id: &str,
    session: &AgentSession,
) -> Result<bool> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(false);
    };
    if !root.exists() {
        return Ok(false);
    }
    amq_handle_is_exact_owner_at_root(&root, store_id, session)
}

pub(crate) fn amq_handle_is_exact_owner_at_root(
    root: &Path,
    store_id: &str,
    session: &AgentSession,
) -> Result<bool> {
    let _lock = AmqRegistryLock::acquire(root)?;
    Ok(matches!(
        marker_state(root, session.agent_handle())?,
        MarkerState::Owner(owner)
            if owner.store_id == store_id && owner.session_id == session.id
    ))
}

pub(crate) fn free_amq_handle_at_root(
    root: &Path,
    store_id: &str,
    session: &AgentSession,
) -> Result<()> {
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
            crate::sanitize::for_terminal(&agent_dir.display().to_string())
        )
    })
}

fn exact_owner(root: &Path, store_id: &str, session: &AgentSession) -> Result<OwnerMarker> {
    match marker_state(root, session.agent_handle())? {
        MarkerState::Owner(owner)
            if owner.store_id == store_id && owner.session_id == session.id =>
        {
            Ok(owner)
        }
        MarkerState::Owner(_) | MarkerState::Legacy(_) | MarkerState::Foreign => bail!(
            "refusing AMQ cleanup for handle {:?}: ownership does not match store/session",
            crate::sanitize::for_terminal(session.agent_handle())
        ),
        MarkerState::Free => bail!(
            "refusing AMQ cleanup for handle {:?}: ownership marker is missing",
            crate::sanitize::for_terminal(session.agent_handle())
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
    if !is_amq_wake_process(pid, root, handle) {
        tracing::warn!(
            target: "dux::peer",
            wake_pid = pid,
            "recorded wake PID no longer identifies an AMQ wake process; leaving it untouched"
        );
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

fn is_amq_wake_process(pid: u32, root: &Path, handle: &str) -> bool {
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
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    use crate::model::{ProviderKind, SessionSettings, SessionState};

    fn session(id: &str, provider: &str, branch: &str, worktree: &Path) -> AgentSession {
        AgentSession {
            id: id.to_string(),
            project_id: "project".to_string(),
            project_path: None,
            provider: ProviderKind::new(provider),
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            worktree_path: worktree.display().to_string(),
            agent_handle: crate::model::normalize_agent_handle(branch),
            shared_workspace: false,
            deleted_at: None,
            title: None,
            started_providers: Vec::new(),
            provider_session_ids: Default::default(),
            state: SessionState::Created {
                created_at: Utc::now(),
            },
            settings: SessionSettings::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn subprocess_helper() {
        let Some(mode) = std::env::var_os("DUX_AMQ_TEST_HELPER_MODE") else {
            return;
        };
        let path = |name| PathBuf::from(std::env::var_os(name).expect(name));
        match mode.to_string_lossy().as_ref() {
            "claim" => {
                let root = path("DUX_AMQ_TEST_ROOT");
                let database = path("DUX_AMQ_TEST_DATABASE");
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
                let store = SessionStore::open(&database).unwrap();
                let mut candidate = session(&session_id, "claude", "agent", &worktree);
                candidate.state = SessionState::Spawning { since: Utc::now() };
                reserve_and_persist_session_at_root(&root, &store_id, &store, &mut candidate)
                    .unwrap();
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
                let result = flock(&file, FlockOperation::NonBlockingLockExclusive);
                let message = match result {
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

    #[test]
    fn target_resolution_uses_persisted_agent_handle() {
        let dir = tempdir().unwrap();
        let worktree = dir.path().join("Feature Login");
        fs::create_dir_all(&worktree).unwrap();
        let s = session("s1", "claude", "renamed", &worktree);

        let target = resolve_target("renamed", &[s]).unwrap();

        assert_eq!(target.handle, "renamed");
        assert_eq!(target.session.unwrap().id, "s1");
    }

    #[test]
    fn target_resolution_rejects_unknown_agent() {
        let err = resolve_target("missing-branch", &[])
            .expect_err("unknown targets must fail before transport selection");

        assert_eq!(
            err.to_string(),
            "agent \"missing-branch\" does not exist or is not reachable; run `dux peer list` and retry with a listed handle"
        );
    }

    #[test]
    fn pty_env_exports_store_session_and_immutable_handle() {
        let dir = tempdir().unwrap();
        let worktree = dir.path().join("different-path-name");
        let s = session("session-a", "claude", "stable-handle", &worktree);
        let mut env = PerSessionEnv::empty();

        append_session_env(&mut env, &s, "store-a");

        assert!(
            env.vars
                .contains(&("DUX_STORE_ID".to_string(), "store-a".to_string()))
        );
        assert!(
            env.vars
                .contains(&("DUX_SESSION_ID".to_string(), "session-a".to_string()))
        );
        assert!(
            env.vars
                .contains(&("DUX_AMQ_HANDLE".to_string(), "stable-handle".to_string()))
        );
    }

    #[test]
    fn routable_sessions_exclude_exited_and_missing_worktrees() {
        let dir = tempdir().unwrap();
        let live_wt = dir.path().join("live");
        fs::create_dir_all(&live_wt).unwrap();
        let mut exited = session("s2", "codex", "exited", &live_wt);
        exited.state = SessionState::Exited {
            exit_code: None,
            exited_at: Utc::now(),
        };
        let missing = session("s3", "claude", "missing", &dir.path().join("missing"));

        assert!(is_routable_session(&session(
            "s1", "claude", "live", &live_wt
        )));
        assert!(!is_routable_session(&exited));
        assert!(!is_routable_session(&missing));
    }

    #[test]
    fn explicit_claude_peers_refuses_codex_target() {
        let dir = tempdir().unwrap();
        let sender_wt = dir.path().join("sender");
        let target_wt = dir.path().join("target");
        fs::create_dir_all(&sender_wt).unwrap();
        fs::create_dir_all(&target_wt).unwrap();
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(session("s1", "claude", "sender", &sender_wt)),
        };
        let target = PeerTarget {
            handle: "target".to_string(),
            session: Some(session("s2", "codex", "target", &target_wt)),
        };

        let err = choose_transport(TransportPreference::ClaudePeers, &sender, &target)
            .unwrap_err()
            .to_string();

        assert!(err.contains("cannot target provider codex"));
    }

    #[test]
    fn shared_sender_to_worktree_target_routes_via_amq() {
        let dir = tempdir().unwrap();
        let mut sender_session = session("s1", "claude", "sender", dir.path());
        sender_session.shared_workspace = true;
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(sender_session),
        };
        let target = PeerTarget {
            handle: "target".to_string(),
            session: Some(session("s2", "claude", "target", dir.path())),
        };

        let transport = choose_transport(TransportPreference::Auto, &sender, &target).unwrap();

        assert_eq!(transport, ChosenTransport::Amq);
    }

    #[test]
    fn worktree_sender_to_shared_target_routes_via_amq() {
        let dir = tempdir().unwrap();
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(session("s1", "claude", "sender", dir.path())),
        };
        let mut target_session = session("s2", "claude", "target", dir.path());
        target_session.shared_workspace = true;
        let target = PeerTarget {
            handle: target_session.agent_handle().to_string(),
            session: Some(target_session),
        };

        let transport = choose_transport(TransportPreference::Auto, &sender, &target).unwrap();

        assert_eq!(transport, ChosenTransport::Amq);
        assert_eq!(target.handle, "target");
    }

    #[test]
    fn shared_sender_to_shared_target_routes_via_amq() {
        let dir = tempdir().unwrap();
        let mut sender_session = session("s1", "claude", "sender", dir.path());
        sender_session.shared_workspace = true;
        let mut target_session = session("s2", "claude", "target", dir.path());
        target_session.shared_workspace = true;
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(sender_session),
        };
        let target = PeerTarget {
            handle: "target".to_string(),
            session: Some(target_session),
        };

        let transport = choose_transport(TransportPreference::Auto, &sender, &target).unwrap();

        assert_eq!(transport, ChosenTransport::Amq);
    }

    #[test]
    fn worktree_endpoints_still_prefer_claude_peers() {
        let dir = tempdir().unwrap();
        let sender_wt = dir.path().join("sender");
        let target_wt = dir.path().join("target");
        fs::create_dir_all(&sender_wt).unwrap();
        fs::create_dir_all(&target_wt).unwrap();
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(session("s1", "codex", "sender", &sender_wt)),
        };
        let target = PeerTarget {
            handle: "target".to_string(),
            session: Some(session("s2", "claude", "target", &target_wt)),
        };

        let transport = choose_transport(TransportPreference::Auto, &sender, &target).unwrap();

        assert_eq!(transport, ChosenTransport::ClaudePeers);
    }

    #[test]
    fn explicit_claude_peers_rejects_shared_sender() {
        let dir = tempdir().unwrap();
        let mut sender_session = session("s1", "claude", "sender", dir.path());
        sender_session.shared_workspace = true;
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(sender_session),
        };
        let target = PeerTarget {
            handle: "target".to_string(),
            session: Some(session("s2", "claude", "target", dir.path())),
        };

        let err = choose_transport(TransportPreference::ClaudePeers, &sender, &target)
            .unwrap_err()
            .to_string();

        assert!(err.contains("shared-workspace"));
        assert!(err.contains("--transport amq"));
    }

    #[test]
    fn explicit_claude_peers_rejects_shared_target() {
        let dir = tempdir().unwrap();
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(session("s1", "claude", "sender", dir.path())),
        };
        let mut target_session = session("s2", "claude", "target", dir.path());
        target_session.shared_workspace = true;
        let target = PeerTarget {
            handle: "target".to_string(),
            session: Some(target_session),
        };

        let err = choose_transport(TransportPreference::ClaudePeers, &sender, &target)
            .unwrap_err()
            .to_string();

        assert!(err.contains("shared-workspace"));
        assert!(err.contains("--transport amq"));
    }

    #[test]
    fn ambiguous_cwd_sender_is_rejected_with_from_hint() {
        let dir = tempdir().unwrap();
        let sessions = vec![
            session("s1", "claude", "sender-one", dir.path()),
            session("s2", "claude", "sender-two", dir.path()),
        ];

        let err = session_for_cwd(dir.path(), &sessions)
            .unwrap_err()
            .to_string();

        assert!(err.contains("multiple Dux sessions share this workspace"));
        assert!(err.contains("--from <agent_handle>"));
    }

    #[test]
    fn cwd_sender_still_prefers_the_deepest_worktree() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("nested");
        let child = nested.join("src");
        fs::create_dir_all(&child).unwrap();
        let sessions = vec![
            session("parent", "claude", "parent", dir.path()),
            session("nested", "claude", "nested", &nested),
        ];

        let sender = session_for_cwd(&child, &sessions).unwrap().unwrap();

        assert_eq!(sender.id, "nested");
    }

    #[test]
    fn explicit_from_resolves_exact_agent_handle_before_aliases() {
        let dir = tempdir().unwrap();
        let mut alias = session("s1", "claude", "other-handle", dir.path());
        alias.branch_name = "stable-handle".to_string();
        let exact = session("s2", "claude", "stable-handle", dir.path());

        let sender = infer_sender(Some("stable-handle"), &[alias, exact]).unwrap();

        assert_eq!(sender.handle, "stable-handle");
        assert_eq!(sender.session.unwrap().id, "s2");
    }

    #[test]
    fn non_claude_sender_gets_dux_reply_hint_for_claude_peers() {
        let dir = tempdir().unwrap();
        let sender_wt = dir.path().join("sender");
        fs::create_dir_all(&sender_wt).unwrap();
        let sender = SenderContext {
            handle: "sender".to_string(),
            session: Some(session("s1", "codex", "sender", &sender_wt)),
        };

        let (from_id, message) = claude_peers_sender(&sender, &[], "status?");

        assert_eq!(from_id, "sender");
        assert!(message.contains("dux peer send sender"));
        assert!(message.ends_with("status?"));
    }

    #[test]
    fn send_arg_parser_allows_message_words_that_look_like_flags() {
        let args = vec![
            "--transport".to_string(),
            "amq".to_string(),
            "worker".to_string(),
            "--please".to_string(),
            "respond".to_string(),
        ];

        let parsed = parse_send_args(&args).unwrap();

        assert_eq!(parsed.target, "worker");
        assert_eq!(parsed.message, "--please respond");
        assert_eq!(parsed.transport, TransportPreference::Amq);
    }

    #[test]
    fn amq_sync_adds_session_handles_to_config_and_agent_dirs() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        let store = SessionStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        let worktree = dir.path().join("worktrees/Agent One");
        fs::create_dir_all(&worktree).unwrap();
        let s = session("s1", "claude", "agent-one", &worktree);
        store.upsert_session(&s).unwrap();
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
        let raw = fs::read_to_string(root.join("meta/config.json")).unwrap();
        assert!(raw.contains("\"agent-one\""));
        let owner: OwnerMarker = serde_json::from_str(
            &fs::read_to_string(root.join("agents/agent-one/.dux-amq-source")).unwrap(),
        )
        .unwrap();
        assert_eq!(owner.store_id, "store-a");
        assert_eq!(owner.session_id, "s1");
    }

    #[test]
    fn amq_sync_prunes_only_missing_rows_from_its_own_store() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        let store = SessionStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        fs::create_dir_all(root.join("meta")).unwrap();
        fs::create_dir_all(root.join("agents/manual")).unwrap();
        fs::write(
            root.join("meta/config.json"),
            r#"{"version":1,"created_utc":"now","agents":["foreign","manual","stale"]}"#,
        )
        .unwrap();
        ensure_owner_marker(
            &root,
            "foreign",
            &OwnerMarker {
                store_id: "store-b".to_string(),
                session_id: "foreign-session".to_string(),
                wake_pid: None,
            },
        )
        .unwrap();
        ensure_owner_marker(
            &root,
            "stale",
            &OwnerMarker {
                store_id: "store-a".to_string(),
                session_id: "missing-session".to_string(),
                wake_pid: None,
            },
        )
        .unwrap();

        let report = reconcile_amq_root(&root, "store-a", Some(&store), &mut []).unwrap();
        let raw = fs::read_to_string(root.join("meta/config.json")).unwrap();

        assert_eq!(report.stale_config_agents_removed, 1);
        assert!(raw.contains("\"manual\""));
        assert!(raw.contains("\"foreign\""));
        assert!(!raw.contains("\"stale\""));
    }

    #[test]
    fn global_handle_collision_across_two_stores_gets_suffix() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        let store_a = SessionStore::open(&dir.path().join("a.sqlite3")).unwrap();
        let store_b = SessionStore::open(&dir.path().join("b.sqlite3")).unwrap();
        let worktree_a = dir.path().join("a/agent");
        let worktree_b = dir.path().join("b/agent");
        fs::create_dir_all(&worktree_a).unwrap();
        fs::create_dir_all(&worktree_b).unwrap();
        let mut first = session("a-session", "claude", "agent", &worktree_a);
        let mut second = session("b-session", "codex", "agent", &worktree_b);
        first.state = SessionState::Spawning { since: Utc::now() };
        second.state = SessionState::Spawning { since: Utc::now() };

        reserve_and_persist_session_at_root(&root, "store-a", &store_a, &mut first).unwrap();
        reserve_and_persist_session_at_root(&root, "store-b", &store_b, &mut second).unwrap();

        assert_eq!(first.agent_handle(), "agent");
        assert_eq!(second.agent_handle(), "agent-2");
        assert_eq!(
            store_b.load_sessions().unwrap()[0].agent_handle(),
            "agent-2"
        );
        assert!(root.join("agents/agent").is_dir());
        assert!(root.join("agents/agent-2").is_dir());
    }

    #[test]
    fn concurrent_same_handle_claims_deconflict_under_the_shared_lock() {
        use std::process::Stdio;

        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        let start = dir.path().join("start");
        let gate = AmqRegistryLock::acquire(&root).unwrap();
        let executable = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        let mut databases = Vec::new();
        let mut ready_paths = Vec::new();
        let mut output_paths = Vec::new();
        for label in ["a", "b"] {
            let database = dir.path().join(format!("{label}.sqlite3"));
            let worktree = dir.path().join(format!("{label}/agent"));
            let ready = dir.path().join(format!("{label}.ready"));
            let output = dir.path().join(format!("{label}.handle"));
            let child = Command::new(&executable)
                .arg("subprocess_helper")
                .arg("--nocapture")
                .env("DUX_AMQ_TEST_HELPER_MODE", "claim")
                .env("DUX_AMQ_TEST_ROOT", &root)
                .env("DUX_AMQ_TEST_DATABASE", &database)
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
            databases.push(database);
            ready_paths.push(ready);
            output_paths.push(output);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while ready_paths.iter().any(|path| !path.exists()) {
            assert!(
                Instant::now() < deadline,
                "claim helpers did not become ready"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        fs::write(&start, b"start").unwrap();
        std::thread::sleep(Duration::from_millis(100));
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
        let mut persisted = databases
            .iter()
            .map(|path| {
                SessionStore::open(path).unwrap().load_sessions().unwrap()[0]
                    .agent_handle()
                    .to_string()
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
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("agents")).unwrap();
        let base = "a".repeat(crate::model::AGENT_HANDLE_MAX_LEN);
        let owner = OwnerMarker {
            store_id: "store-a".to_string(),
            session_id: "session-a".to_string(),
            wake_pid: None,
        };

        let handle = next_global_handle(&root, &base, &HashSet::new(), &owner).unwrap();

        assert_eq!(handle.chars().count(), crate::model::AGENT_HANDLE_MAX_LEN);
        assert!(handle.ends_with("-2"));
    }

    #[test]
    fn failed_owner_marker_write_removes_the_new_reservation_dir() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("agents")).unwrap();
        let owner = OwnerMarker {
            store_id: "store-a".to_string(),
            session_id: "session-a".to_string(),
            wake_pid: None,
        };

        let result = ensure_owner_marker_with(&root, "agent", &owner, |_, _| {
            Err(anyhow!("injected marker write failure"))
        });

        assert!(result.is_err());
        assert!(!root.join("agents/agent").exists());
    }

    #[test]
    fn unambiguous_legacy_path_marker_upgrades_to_exact_owner() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        let store = SessionStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        let worktree = dir.path().join("worktree/agent");
        fs::create_dir_all(root.join("agents/agent")).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(&worktree, root.join("agents/agent/.dux-amq-source")).unwrap();
        let s = session("session-a", "claude", "agent", &worktree);
        store.upsert_session(&s).unwrap();
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
        let store = SessionStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        let worktree = dir.path().join("shared-worktree");
        fs::create_dir_all(root.join("agents/agent")).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        std::os::unix::fs::symlink(&worktree, root.join("agents/agent/.dux-amq-source")).unwrap();
        let first = session("a-session", "claude", "agent", &worktree);
        let second = session("b-session", "codex", "other", &worktree);
        store.upsert_session(&first).unwrap();
        store.upsert_session(&second).unwrap();
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

    #[test]
    fn exact_owner_free_rejects_foreign_then_removes_owned_inbox() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("agents")).unwrap();
        let worktree = dir.path().join("agent");
        fs::create_dir_all(&worktree).unwrap();
        let s = session("session-a", "claude", "agent", &worktree);
        ensure_owner_marker(
            &root,
            "agent",
            &OwnerMarker {
                store_id: "store-a".to_string(),
                session_id: s.id.clone(),
                wake_pid: None,
            },
        )
        .unwrap();
        register_config_handle(&root, "agent").unwrap();

        assert!(free_amq_handle_at_root(&root, "store-b", &s).is_err());
        assert!(root.join("agents/agent").exists());
        free_amq_handle_at_root(&root, "store-a", &s).unwrap();
        assert!(!root.join("agents/agent").exists());
        assert!(
            !fs::read_to_string(root.join("meta/config.json"))
                .unwrap()
                .contains("\"agent\"")
        );
    }

    #[test]
    fn exact_owner_free_sanitizes_removal_error_path() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let root = dir.path().join("amq-\u{1b}]8;;bad\u{7}");
        let agents = root.join("agents");
        fs::create_dir_all(&agents).unwrap();
        let worktree = dir.path().join("agent");
        fs::create_dir_all(&worktree).unwrap();
        let session = session("session-a", "claude", "agent", &worktree);
        ensure_owner_marker(
            &root,
            "agent",
            &OwnerMarker {
                store_id: "store-a".to_string(),
                session_id: session.id.clone(),
                wake_pid: None,
            },
        )
        .unwrap();
        register_config_handle(&root, "agent").unwrap();

        fs::set_permissions(&agents, fs::Permissions::from_mode(0o500)).unwrap();
        let error = free_amq_handle_at_root(&root, "store-a", &session).unwrap_err();
        fs::set_permissions(&agents, fs::Permissions::from_mode(0o700)).unwrap();

        let message = format!("{error:#}");
        assert!(message.contains("failed to remove owned AMQ inbox"));
        assert!(!message.contains('\u{1b}'));
    }

    #[test]
    fn spawn_failure_after_persist_leaves_retryable_owned_row() {
        let dir = tempdir().unwrap();
        let dux_home = dir.path().join("store");
        let amq_root = dir.path().join("amq");
        fs::create_dir_all(&amq_root).unwrap();
        fs::create_dir_all(&dux_home).unwrap();
        let paths = DuxPaths {
            config_path: dux_home.join("config.toml"),
            sessions_db_path: dux_home.join("sessions.sqlite3"),
            worktrees_root: dux_home.join("worktrees"),
            lock_path: dux_home.join("dux.lock"),
            root: dux_home,
        };
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        let worktree = dir.path().join("worktree/agent");
        fs::create_dir_all(&worktree).unwrap();
        let mut s = session("session-a", "claude", "agent", &worktree);
        s.state = SessionState::Spawning { since: Utc::now() };

        reserve_and_persist_session_at_root(&amq_root, "store-a", &store, &mut s).unwrap();
        s.state = SessionState::Retryable {
            interrupted_at: Utc::now(),
        };
        store.upsert_session(&s).unwrap();

        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].state.is_retryable());
        let owner = match marker_state(&amq_root, loaded[0].agent_handle()).unwrap() {
            MarkerState::Owner(owner) => owner,
            _ => panic!("reserved owner marker missing"),
        };
        assert_eq!(owner.store_id, "store-a");
        assert_eq!(owner.session_id, "session-a");
    }

    #[test]
    fn rust_and_wrapper_claims_serialize_on_config_lock() {
        use std::process::Stdio;

        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        let home = dir.path().join("home");
        let worktree = dir.path().join("worktree");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&worktree).unwrap();
        let lock = AmqRegistryLock::acquire(&root).unwrap();
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let tests = manifest.join("dux-amq/tests");
        let path = format!(
            "{}:{}:{}",
            tests.join("fakes").display(),
            manifest.join("dux-amq/scripts").display(),
            env::var("PATH").unwrap_or_default()
        );
        let mut child = Command::new(manifest.join("dux-amq/wrappers/claude-amq"))
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
        use std::process::Stdio;

        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("agents")).unwrap();
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
        let worktree = dir.path().join("agent");
        fs::create_dir_all(&worktree).unwrap();
        let session = session("session-a", "claude", "agent", &worktree);
        ensure_owner_marker(
            &root,
            "agent",
            &OwnerMarker {
                store_id: "store-a".to_string(),
                session_id: session.id.clone(),
                wake_pid: Some(wake_pid),
            },
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
                // positively identify by argv (`amq wake --me <handle> --root
                // <root>`). Some sandboxed environments (notably the GitHub
                // macOS CI runner) cannot read another process's argv via
                // sysinfo, so the guard conservatively skips the kill — a safe
                // leak, not a wrong-kill. In that case there is no SIGTERM to
                // observe; self-skip rather than fail. The Linux CI leg (the
                // AMQ deployment platform) exercises the real termination path.
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
            .arg("subprocess_helper")
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
        use std::os::unix::fs::PermissionsExt;
        use std::process::Stdio;

        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("agents")).unwrap();
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
        let worktree = dir.path().join("agent");
        fs::create_dir_all(&worktree).unwrap();
        let s = session("session-a", "claude", "agent", &worktree);
        ensure_owner_marker(
            &root,
            "agent",
            &OwnerMarker {
                store_id: "store-a".to_string(),
                session_id: s.id.clone(),
                wake_pid: Some(pid),
            },
        )
        .unwrap();
        register_config_handle(&root, "agent").unwrap();

        tombstone_amq_session_at_root(&root, "store-a", &s).unwrap();
        let _ = reaper.join().unwrap();

        assert!(root.join("agents/agent").is_dir());
        let owner = match marker_state(&root, "agent").unwrap() {
            MarkerState::Owner(owner) => owner,
            _ => panic!("owner marker missing"),
        };
        assert_eq!(owner.wake_pid, None);
        assert!(
            !fs::read_to_string(root.join("meta/config.json"))
                .unwrap()
                .contains("\"agent\"")
        );
    }
}
