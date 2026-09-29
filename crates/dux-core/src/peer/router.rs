//! `dux peer`: route a message between Dux agent sessions over the right
//! transport, so agents never pick between AMQ and Claude Peers themselves.
//!
//! Transport rules (the fork's, unchanged):
//!
//! - Either endpoint in a shared workspace: always AMQ. Claude Peers keys a
//!   registration by working directory, so it cannot tell the agents sharing
//!   one apart, and an explicit `--transport claude-peers` is refused.
//! - Otherwise `auto` sends to a Claude target over Claude Peers (the broker
//!   is then required, not a best-effort preference) and to anything else
//!   over AMQ. `--transport amq` is the manual override.
//!
//! Only live sessions whose directory still exists are listed or routable.

use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use super::PeerSession;
use super::amq::{require_amq_root, sync_amq_agents};
use super::handle::{amq_handle, truncate, utf8_lossy};
use crate::config::DuxPaths;

const DEFAULT_CLAUDE_PEERS_PORT: u16 = 7899;

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
    session: Option<PeerSession>,
}

#[derive(Clone, Debug)]
struct SenderContext {
    handle: String,
    session: Option<PeerSession>,
}

#[derive(Clone, Debug, Deserialize)]
struct ClaudePeer {
    id: String,
    cwd: String,
    /// Peer process id. Absent on brokers that predate the field; the
    /// registration then ranks as an ordinary client, decided by recency.
    #[serde(default)]
    pid: Option<u32>,
    /// Broker heartbeat, ISO-8601. Lexicographically sortable.
    #[serde(default)]
    last_seen: Option<String>,
    /// When this registration was created, ISO-8601. Continuing or forking a
    /// conversation registers afresh, so among equals the newest is current.
    #[serde(default)]
    registered_at: Option<String>,
}

pub const PEER_HELP: &str = "\
dux peer - route messages between Dux agent sessions

Subcommands:
  dux peer send [--from <handle>] [--transport auto|amq|claude-peers] <target> <message...>
                       Send through the Dux router. Auto uses AMQ when either
                       endpoint is shared; otherwise Claude targets use Peers.
  dux peer list        List Dux sessions and transport health.
  dux peer sync-amq    Reconcile AMQ's agent registry from sessions.sqlite3.

Agents should use this command instead of calling amq or Claude Peers directly.";

/// Entry point for `dux peer <args>`.
pub fn run_peer(args: &[String], paths: &DuxPaths) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "send" => run_peer_send(&args[1..], paths),
        "list" => run_peer_list(&args[1..], paths),
        "sync-amq" | "sync" => run_peer_sync_amq(&args[1..], paths),
        "" | "--help" | "-h" => {
            println!("{PEER_HELP}");
            Ok(())
        }
        other => bail!("unknown peer subcommand: {other}\nRun `dux peer --help` for usage."),
    }
}

fn run_peer_send(args: &[String], paths: &DuxPaths) -> Result<()> {
    let Some(parsed) = parse_send_args(args)? else {
        println!("{PEER_HELP}");
        return Ok(());
    };
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
            let dux_pid = dux_tui_pid(paths);
            let to_id =
                claude_peer_id_for_session(target_session, &peers, dux_pid).ok_or_else(|| {
                    anyhow!(
                        "Claude Peers is not registered for target {}",
                        target_session.agent_handle()
                    )
                })?;
            let (from_id, message) = claude_peers_sender(&sender, &peers, &parsed.message, dux_pid);
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
                session.agent_handle(),
                session.provider,
                session.directory
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
    let Some(root) = report.root.as_ref().filter(|_| !report.skipped) else {
        println!("AMQ sync skipped: no configured AMQ root found");
        return Ok(());
    };
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

#[derive(Debug)]
struct SendArgs {
    from: Option<String>,
    transport: TransportPreference,
    target: String,
    message: String,
}

/// Parse `dux peer send` arguments. `Ok(None)` means help was requested.
/// Flags are only recognised before the target: everything after it is the
/// message, so message words that look like flags are sent verbatim.
fn parse_send_args(args: &[String]) -> Result<Option<SendArgs>> {
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
            "-h" | "--help" => return Ok(None),
            arg if arg.starts_with('-') => bail!("unknown flag: {arg}"),
            _ => positional.push(args[i].clone()),
        }
        i += 1;
    }

    if positional.len() < 2 {
        bail!("usage: dux peer send <target> <message...>");
    }
    Ok(Some(SendArgs {
        from,
        transport,
        target: positional[0].clone(),
        message: positional[1..].join(" "),
    }))
}

fn reject_unknown_peer_flags(args: &[String]) -> Result<()> {
    for arg in args {
        if arg.starts_with('-') {
            bail!("unknown flag: {arg}");
        }
    }
    Ok(())
}

fn load_sessions_if_present(paths: &DuxPaths) -> Result<Vec<PeerSession>> {
    if !paths.sessions_db_path.exists() {
        return Ok(Vec::new());
    }
    let sessions = super::session_store::open(paths)?
        .load_sessions()
        .context("failed to load Dux sessions")?;
    Ok(sessions
        .into_iter()
        .filter(PeerSession::is_routable)
        .collect())
}

fn infer_sender(from: Option<&str>, sessions: &[PeerSession]) -> Result<SenderContext> {
    if let Some(from) = from {
        let handle = amq_handle(from);
        if handle.is_empty() {
            bail!("--from normalizes to an empty AMQ handle");
        }
        // An exact handle beats any alias, so another session whose branch
        // happens to equal this handle cannot impersonate it.
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
            handle: session.agent_handle().to_string(),
            session: Some(session.clone()),
        });
    }

    for var in ["DUX_AMQ_HANDLE", "AM_ME"] {
        if let Ok(value) = env::var(var) {
            let handle = amq_handle(&value);
            if !handle.is_empty() {
                let session = sessions
                    .iter()
                    .find(|session| session.agent_handle() == handle)
                    .cloned();
                return Ok(SenderContext { handle, session });
            }
        }
    }

    if let Ok(cwd) = env::current_dir()
        && let Some(session) = session_for_cwd(&cwd, sessions)?
    {
        return Ok(SenderContext {
            handle: session.agent_handle().to_string(),
            session: Some(session.clone()),
        });
    }

    Ok(SenderContext {
        handle: "dux-router".to_string(),
        session: None,
    })
}

fn resolve_target(target: &str, sessions: &[PeerSession]) -> Result<PeerTarget> {
    let sanitized = amq_handle(target);
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
            .map(|session| format!("{} ({})", session.agent_handle(), session.id))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("ambiguous peer target {target:?}; matches: {names}");
    }

    if let Some(session) = matches.pop() {
        return Ok(PeerTarget {
            handle: session.agent_handle().to_string(),
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
        .is_some_and(|session| session.shared_workspace)
        || target
            .session
            .as_ref()
            .is_some_and(|session| session.shared_workspace);
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

fn is_claude_session(session: Option<&PeerSession>) -> bool {
    session.is_some_and(PeerSession::is_claude)
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
        bail!("amq send failed: {}", utf8_lossy(&output.stderr));
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

/// Who a Claude Peers message comes from. A Claude sender registered with the
/// broker sends as itself; anyone else sends under its Dux handle with a
/// preamble telling the recipient to reply through `dux peer send`, because
/// the broker cannot deliver back to a non-peer.
fn claude_peers_sender(
    sender: &SenderContext,
    peers: &[ClaudePeer],
    message: &str,
    dux_pid: Option<u32>,
) -> (String, String) {
    if let Some(session) = sender.session.as_ref()
        && session.is_claude()
        && let Some(id) = claude_peer_id_for_session(session, peers, dux_pid)
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
    parse_broker_response(&response)
}

fn parse_broker_response(response: &str) -> Result<Value> {
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
            truncate(response_body, 300)
        );
    }
    serde_json::from_str(response_body).context("Claude Peers broker returned non-JSON response")
}

/// Depth cap when walking a process's ancestry. Panes and daemon hosts sit a
/// handful of levels below their owner; the cap only stops a walk that a
/// pid-reuse cycle would otherwise make unbounded.
const MAX_ANCESTRY_DEPTH: usize = 32;

/// What kind of process owns a Claude Peers registration, ranked by how
/// likely it is to be the conversation a human or agent is actually driving.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PeerHost {
    /// A Claude Code daemon `bg-spare`: pre-warmed, nobody reads it.
    Spare,
    /// Anything unrecognised, such as a plain `claude` in a terminal.
    Client,
    /// The process dux spawned in the pane.
    Pane,
    /// A daemon-hosted session (`claude bg-pty-host ... --session-id`): once
    /// Claude Code continues or forks a pane's conversation, this is where
    /// the turns happen while the pane's original process lingers.
    DaemonSession,
}

fn claude_peer_id_for_session(
    session: &PeerSession,
    peers: &[ClaudePeer],
    dux_pid: Option<u32>,
) -> Option<String> {
    select_claude_peer_id(session, peers, peer_host_classifier(dux_pid))
}

/// Choose which Claude Peers registration owns `session`.
///
/// The working directory alone is not a key: the Claude Code daemon keeps
/// pre-warmed `bg-spare` sessions registered from an agent's own worktree,
/// and when a pane's conversation is continued or forked the daemon hosts
/// the new session in its own process while the pane's original process
/// stays registered with a queue nobody drains. Picking by list order, by
/// heartbeat, or by descent from the dux TUI each landed messages in one of
/// those dead registrations.
///
/// So rank by host kind (daemon session, then pane, then anything else,
/// spares last), then by newest registration, then by heartbeat so the
/// choice stays deterministic.
fn select_claude_peer_id(
    session: &PeerSession,
    peers: &[ClaudePeer],
    host_of: impl Fn(u32) -> PeerHost,
) -> Option<String> {
    let session_path = canonical_or_raw(Path::new(&session.directory));
    let candidates: Vec<&ClaudePeer> = peers
        .iter()
        .filter(|peer| canonical_or_raw(Path::new(&peer.cwd)) == session_path)
        .collect();
    if candidates.len() <= 1 {
        return candidates.first().map(|peer| peer.id.clone());
    }

    let key = |peer: &ClaudePeer| {
        (
            peer.pid.map(&host_of).unwrap_or(PeerHost::Client),
            peer.registered_at.clone(),
            peer.last_seen.clone(),
        )
    };
    candidates
        .iter()
        .max_by(|a, b| key(a).cmp(&key(b)))
        .map(|peer| peer.id.clone())
}

/// Snapshot the process table once and classify a registration's owner by
/// walking its ancestry: the registered pid is the peer's MCP server, whose
/// parents reveal whether a daemon spare, a daemon session host, or the dux
/// TUI is behind it.
fn peer_host_classifier(dux_pid: Option<u32>) -> impl Fn(u32) -> PeerHost {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
    );
    move |pid: u32| {
        let mut current = sysinfo::Pid::from_u32(pid);
        for _ in 0..MAX_ANCESTRY_DEPTH {
            if dux_pid == Some(current.as_u32()) {
                return PeerHost::Pane;
            }
            let Some(proc) = system.process(current) else {
                return PeerHost::Client;
            };
            if let Some(kind) = daemon_host_kind(proc.cmd()) {
                return kind;
            }
            match proc.parent() {
                Some(parent) => current = parent,
                None => return PeerHost::Client,
            }
        }
        PeerHost::Client
    }
}

/// Recognise the Claude Code daemon's process roles from a command line.
/// `bg-spare` is checked first because a spare runs underneath its own
/// `bg-pty-host`, and it is the spare that must lose.
fn daemon_host_kind(command: &[OsString]) -> Option<PeerHost> {
    let has = |needle: &str| command.iter().any(|arg| arg == needle);
    if has("bg-spare") || has("--bg-spare") {
        Some(PeerHost::Spare)
    } else if has("bg-pty-host") || has("--bg-pty-host") {
        Some(PeerHost::DaemonSession)
    } else {
        None
    }
}

/// PID of the dux process holding this config directory, from its lockfile.
fn dux_tui_pid(paths: &DuxPaths) -> Option<u32> {
    fs::read_to_string(&paths.lock_path)
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// The session whose directory contains `cwd`, preferring the deepest one.
/// Two sessions at the same depth (a shared workspace) make the sender
/// ambiguous, which is an error rather than a guess.
fn session_for_cwd<'a>(cwd: &Path, sessions: &'a [PeerSession]) -> Result<Option<&'a PeerSession>> {
    let cwd = canonical_or_raw(cwd);
    let mut nearest = None;
    let mut ambiguous = false;
    for session in sessions {
        let path = canonical_or_raw(Path::new(&session.directory));
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

/// Every name a session answers to as a target: id, handle, branch, title,
/// and its directory's name (raw and sanitised).
fn session_aliases(session: &PeerSession) -> HashSet<String> {
    let mut aliases = HashSet::new();
    aliases.insert(session.id.clone());
    aliases.insert(session.agent_handle().to_string());
    if let Some(branch) = &session.branch {
        aliases.insert(branch.clone());
    }
    if let Some(title) = &session.title {
        aliases.insert(title.clone());
    }
    if let Some(name) = Path::new(&session.directory)
        .file_name()
        .and_then(|name| name.to_str())
    {
        aliases.insert(name.to_string());
        let sanitized = amq_handle(name);
        if !sanitized.is_empty() {
            aliases.insert(sanitized);
        }
    }
    aliases
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod tests;
