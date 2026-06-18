use std::collections::{BTreeSet, HashSet};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::DuxPaths;
use crate::model::AgentSession;
use crate::pty::PerSessionEnv;
use crate::storage::SessionStore;

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
    pub source_links_created: usize,
    pub source_links_replaced: usize,
    pub source_link_conflicts: Vec<String>,
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

pub fn append_session_env(env: &mut PerSessionEnv, session: &AgentSession) {
    env.vars
        .push(("DUX_SESSION_ID".to_string(), session.id.clone()));
    env.vars.push((
        "DUX_PROVIDER".to_string(),
        session.provider.as_str().to_string(),
    ));
    env.vars.push((
        "DUX_AMQ_HANDLE".to_string(),
        amq_handle_for_session(session),
    ));
}

pub fn sync_amq_agents(paths: &DuxPaths, sessions: &[AgentSession]) -> Result<AmqSyncReport> {
    let Some(root) = optional_amq_root(paths) else {
        return Ok(AmqSyncReport {
            skipped: true,
            ..AmqSyncReport::default()
        });
    };
    reconcile_amq_root(&root, sessions)
}

fn run_peer_send(args: &[String], paths: &DuxPaths) -> Result<()> {
    let parsed = parse_send_args(args)?;
    let sessions = load_sessions_if_present(paths)?;
    let _ = sync_amq_agents(paths, &sessions)?;

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
    let sessions = load_sessions_if_present(paths)?;
    let report = sync_amq_agents(paths, &sessions)?;

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
            "AMQ registry: {} (added {}, removed stale {}, link conflicts {})",
            root.display(),
            report.configured_agents_added,
            report.stale_config_agents_removed,
            report.source_link_conflicts.len()
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
    let sessions = load_sessions_if_present(paths)?;
    let report = sync_amq_agents(paths, &sessions)?;
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
    println!("  source links created: {}", report.source_links_created);
    println!("  source links replaced: {}", report.source_links_replaced);
    if !report.source_link_conflicts.is_empty() {
        println!("  source link conflicts:");
        for conflict in report.source_link_conflicts {
            println!("    {conflict}");
        }
    }
    Ok(())
}

fn print_peer_help() {
    println!(
        "\
dux peer - route messages between Dux agent sessions

Subcommands:
  dux peer send [--from <handle>] [--transport auto|amq|claude-peers] <target> <message...>
                       Send through the Dux router. Auto uses Claude Peers for
                       Claude targets and AMQ for non-Claude targets.
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
    SessionStore::open(&paths.sessions_db_path)
        .with_context(|| format!("failed to open {}", paths.sessions_db_path.display()))?
        .load_sessions()
        .context("failed to load Dux sessions")
}

fn infer_sender(from: Option<&str>, sessions: &[AgentSession]) -> Result<SenderContext> {
    if let Some(from) = from {
        let handle = sanitise_handle(from);
        if handle.is_empty() {
            bail!("--from normalizes to an empty AMQ handle");
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
        && let Some(session) = session_for_cwd(&cwd, sessions)
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
    Ok(PeerTarget {
        handle: sanitized,
        session: None,
    })
}

fn choose_transport(
    preference: TransportPreference,
    _sender: &SenderContext,
    target: &PeerTarget,
) -> Result<ChosenTransport> {
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

fn session_for_cwd<'a>(cwd: &Path, sessions: &'a [AgentSession]) -> Option<&'a AgentSession> {
    let cwd = canonical_or_raw(cwd);
    sessions
        .iter()
        .filter_map(|session| {
            let path = canonical_or_raw(Path::new(&session.worktree_path));
            if cwd == path || cwd.starts_with(&path) {
                Some((path.components().count(), session))
            } else {
                None
            }
        })
        .max_by_key(|(depth, _)| *depth)
        .map(|(_, session)| session)
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
    if let Some(name) = Path::new(&session.worktree_path)
        .file_name()
        .and_then(|name| name.to_str())
    {
        let handle = sanitise_handle(name);
        if !handle.is_empty() {
            return handle;
        }
    }
    let handle = sanitise_handle(&session.branch_name);
    if handle.is_empty() {
        sanitise_handle(&session.id)
    } else {
        handle
    }
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
    let data_state = PathBuf::from("/data/state/amq");
    if data_state.exists() {
        return Some(data_state);
    }
    None
}

fn require_amq_root(paths: &DuxPaths) -> Result<PathBuf> {
    optional_amq_root(paths).ok_or_else(|| {
        anyhow!("AMQ root is not configured; set AMQ_GLOBAL_ROOT or install dux-amq")
    })
}

fn reconcile_amq_root(root: &Path, sessions: &[AgentSession]) -> Result<AmqSyncReport> {
    fs::create_dir_all(root.join("meta"))
        .with_context(|| format!("failed to create {}", root.join("meta").display()))?;
    fs::create_dir_all(root.join("agents"))
        .with_context(|| format!("failed to create {}", root.join("agents").display()))?;

    let mut report = AmqSyncReport {
        root: Some(root.to_path_buf()),
        ..AmqSyncReport::default()
    };

    let mut desired = BTreeSet::new();
    for session in sessions {
        let handle = amq_handle_for_session(session);
        if handle.is_empty() {
            continue;
        }
        desired.insert(handle.clone());
        let agent_dir = root.join("agents").join(&handle);
        fs::create_dir_all(&agent_dir)
            .with_context(|| format!("failed to create {}", agent_dir.display()))?;
        reconcile_source_link(
            &agent_dir,
            Path::new(&session.worktree_path),
            &handle,
            &mut report,
        )?;
    }

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
    for handle in &desired {
        agents.insert(handle.clone());
    }
    report.configured_agents_added = agents.len().saturating_sub(before);

    let before_prune = agents.len();
    agents.retain(|handle| desired.contains(handle) || !is_stale_dux_owned_agent(root, handle));
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
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let body = serde_json::to_string_pretty(value)?;
    fs::write(&tmp, format!("{body}\n"))
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| {
        format!(
            "failed to replace {} with {}",
            path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

fn reconcile_source_link(
    agent_dir: &Path,
    worktree: &Path,
    handle: &str,
    report: &mut AmqSyncReport,
) -> Result<()> {
    let marker = agent_dir.join(".dux-amq-source");
    let desired = worktree.to_path_buf();
    if path_exists_or_symlink(&marker) {
        let previous = read_source_marker(&marker)?;
        if paths_equivalent(&previous, &desired) {
            return Ok(());
        }
        if previous.is_absolute() && !previous.exists() {
            fs::remove_file(&marker)
                .with_context(|| format!("failed to remove stale {}", marker.display()))?;
            create_source_marker(&marker, &desired)?;
            report.source_links_replaced += 1;
            return Ok(());
        }
        report.source_link_conflicts.push(format!(
            "{handle}: {} already points to {}",
            marker.display(),
            previous.display()
        ));
        return Ok(());
    }

    create_source_marker(&marker, &desired)?;
    report.source_links_created += 1;
    Ok(())
}

fn read_source_marker(path: &Path) -> Result<PathBuf> {
    if path.symlink_metadata()?.file_type().is_symlink() {
        return fs::read_link(path)
            .with_context(|| format!("failed to read symlink {}", path.display()));
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(PathBuf::from(text.trim()))
}

fn create_source_marker(path: &Path, target: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, path).with_context(|| {
            format!(
                "failed to symlink {} -> {}",
                path.display(),
                target.display()
            )
        })
    }
    #[cfg(not(unix))]
    {
        fs::write(path, target.display().to_string()).with_context(|| {
            format!(
                "failed to write {} for target {}",
                path.display(),
                target.display()
            )
        })
    }
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

fn is_stale_dux_owned_agent(root: &Path, handle: &str) -> bool {
    let marker = root.join("agents").join(handle).join(".dux-amq-source");
    if !path_exists_or_symlink(&marker) {
        return false;
    }
    let Ok(previous) = read_source_marker(&marker) else {
        return false;
    };
    previous.is_absolute() && !previous.exists()
}

fn path_exists_or_symlink(path: &Path) -> bool {
    path.exists() || fs::symlink_metadata(path).is_ok()
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
            title: None,
            started_providers: Vec::new(),
            state: SessionState::Created {
                created_at: Utc::now(),
            },
            settings: SessionSettings::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn target_resolution_matches_sanitized_worktree_basename() {
        let dir = tempdir().unwrap();
        let worktree = dir.path().join("Feature Login");
        fs::create_dir_all(&worktree).unwrap();
        let s = session("s1", "claude", "renamed", &worktree);

        let target = resolve_target("feature-login", &[s]).unwrap();

        assert_eq!(target.handle, "feature-login");
        assert_eq!(target.session.unwrap().id, "s1");
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
    fn auto_uses_claude_peers_for_claude_targets_from_any_sender() {
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
        let worktree = dir.path().join("worktrees/Agent One");
        fs::create_dir_all(&worktree).unwrap();
        let s = session("s1", "claude", "agent-one", &worktree);

        let report = reconcile_amq_root(&root, &[s]).unwrap();

        assert_eq!(report.configured_agents_added, 1);
        assert!(root.join("agents/agent-one").is_dir());
        let raw = fs::read_to_string(root.join("meta/config.json")).unwrap();
        assert!(raw.contains("\"agent-one\""));
        assert!(root.join("agents/agent-one/.dux-amq-source").exists());
    }

    #[test]
    fn amq_sync_prunes_stale_dux_owned_config_agents_only() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("amq");
        fs::create_dir_all(root.join("meta")).unwrap();
        fs::create_dir_all(root.join("agents/stale")).unwrap();
        fs::create_dir_all(root.join("agents/manual")).unwrap();
        fs::write(
            root.join("meta/config.json"),
            r#"{"version":1,"created_utc":"now","agents":["manual","stale"]}"#,
        )
        .unwrap();
        create_source_marker(
            &root.join("agents/stale/.dux-amq-source"),
            &dir.path().join("missing"),
        )
        .unwrap();

        let report = reconcile_amq_root(&root, &[]).unwrap();
        let raw = fs::read_to_string(root.join("meta/config.json")).unwrap();

        assert_eq!(report.stale_config_agents_removed, 1);
        assert!(raw.contains("\"manual\""));
        assert!(!raw.contains("\"stale\""));
    }
}
