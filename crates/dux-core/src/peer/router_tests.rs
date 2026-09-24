use super::*;
use std::fs;
use tempfile::tempdir;

use crate::peer::test_support::session;

fn peer(id: &str, cwd: &Path, pid: u32, registered_at: &str) -> ClaudePeer {
    ClaudePeer {
        id: id.to_string(),
        cwd: cwd.to_string_lossy().to_string(),
        pid: Some(pid),
        last_seen: Some("2026-09-16T00:00:00Z".to_string()),
        registered_at: Some(registered_at.to_string()),
    }
}

fn worktree_session(dir: &tempfile::TempDir) -> (PathBuf, PeerSession) {
    let wt = dir.path().join("worktree");
    fs::create_dir_all(&wt).unwrap();
    let session = session("s1", "claude", "feature", &wt);
    (wt, session)
}

/// Observed live: the pane's original process (a dux child, oldest
/// registration) kept queueing channel messages nobody drained after
/// Claude Code continued the conversation into a daemon-hosted session.
/// A pre-warmed spare in the same worktree was newer than both.
#[test]
fn peer_selection_prefers_the_daemon_hosted_session() {
    let dir = tempdir().unwrap();
    let (wt, session) = worktree_session(&dir);
    let peers = vec![
        peer("pane-shell", &wt, 100, "2026-09-15T07:24:00Z"),
        peer("spare", &wt, 200, "2026-09-15T23:00:00Z"),
        peer("daemon-host", &wt, 300, "2026-09-15T21:13:00Z"),
    ];
    let host_of = |pid: u32| match pid {
        100 => PeerHost::Pane,
        200 => PeerHost::Spare,
        300 => PeerHost::DaemonSession,
        _ => PeerHost::Client,
    };
    assert_eq!(
        select_claude_peer_id(&session, &peers, host_of),
        Some("daemon-host".to_string()),
    );
}

/// Without a daemon in the picture the pane dux spawned must still beat
/// a spare listed first and heartbeating more recently.
#[test]
fn peer_selection_prefers_the_pane_over_spares_and_strangers() {
    let dir = tempdir().unwrap();
    let (wt, session) = worktree_session(&dir);
    let peers = vec![
        peer("spare", &wt, 4242, "2026-09-16T00:00:09Z"),
        peer("stranger", &wt, 7, "2026-09-16T00:00:05Z"),
        peer("pane", &wt, 1001, "2026-09-16T00:00:01Z"),
    ];
    let host_of = |pid: u32| match pid {
        1001 => PeerHost::Pane,
        4242 => PeerHost::Spare,
        _ => PeerHost::Client,
    };
    assert_eq!(
        select_claude_peer_id(&session, &peers, host_of),
        Some("pane".to_string()),
    );
}

#[test]
fn peer_selection_takes_newest_registration_among_equal_hosts() {
    let dir = tempdir().unwrap();
    let (wt, session) = worktree_session(&dir);
    let peers = vec![
        peer("old-host", &wt, 1, "2026-09-15T21:13:00Z"),
        peer("new-host", &wt, 2, "2026-09-16T01:00:00Z"),
    ];
    assert_eq!(
        select_claude_peer_id(&session, &peers, |_| PeerHost::DaemonSession),
        Some("new-host".to_string()),
    );
    // Only spares left (TUI gone, or a broker without pids): still
    // deterministic rather than list order.
    assert_eq!(
        select_claude_peer_id(&session, &peers, |_| PeerHost::Spare),
        Some("new-host".to_string()),
    );
}

#[test]
fn peer_selection_ignores_other_worktrees_and_reports_no_match() {
    let dir = tempdir().unwrap();
    let (wt, session) = worktree_session(&dir);
    let other = dir.path().join("other");
    fs::create_dir_all(&other).unwrap();
    let peers = vec![peer("elsewhere", &other, 1001, "2026-09-16T00:00:09Z")];
    assert_eq!(
        select_claude_peer_id(&session, &peers, |_| PeerHost::DaemonSession),
        None
    );
    // A single match needs no disambiguation at all, spare or not.
    let peers = vec![peer("only", &wt, 1, "2026-09-16T00:00:01Z")];
    assert_eq!(
        select_claude_peer_id(&session, &peers, |_| PeerHost::Spare),
        Some("only".to_string()),
    );
}

/// A broker that predates `pid` / `registered_at` still parses, and its
/// registrations rank as ordinary clients.
#[test]
fn broker_peers_without_optional_fields_still_parse() {
    let peers: Vec<ClaudePeer> =
        serde_json::from_value(json!([{ "id": "a", "cwd": "/x" }])).unwrap();
    assert_eq!(peers[0].pid, None);
    assert_eq!(peers[0].registered_at, None);
}

#[test]
fn daemon_roles_are_recognised_from_command_lines() {
    let cmd = |parts: &[&str]| parts.iter().map(OsString::from).collect::<Vec<_>>();
    assert_eq!(
        daemon_host_kind(&cmd(&[
            "claude",
            "bg-spare",
            "--bg-spare",
            "/tmp/x.claim.sock"
        ])),
        Some(PeerHost::Spare)
    );
    assert_eq!(
        daemon_host_kind(&cmd(&[
            "claude",
            "bg-pty-host",
            "--bg-pty-host",
            "/tmp/x.pty.sock",
            "200",
            "50",
            "--",
            "/x/claude",
            "--session-id",
            "11111111-2222-4333-8444-555555555555",
            "--fork-session",
        ])),
        Some(PeerHost::DaemonSession)
    );
    assert_eq!(
        daemon_host_kind(&cmd(&[
            "claude",
            "--dangerously-skip-permissions",
            "--continue"
        ])),
        None
    );
    assert_eq!(daemon_host_kind(&[]), None);
}

/// The classifier walks real process ancestry: a child of this test
/// process, with this process standing in for the dux TUI, is its pane.
#[test]
fn host_classifier_finds_the_dux_pane_by_ancestry() {
    let mut child = Command::new("sleep").arg("5").spawn().unwrap();
    let classify = peer_host_classifier(Some(std::process::id()));
    assert_eq!(classify(child.id()), PeerHost::Pane);
    let unrelated = peer_host_classifier(None);
    assert_eq!(unrelated(child.id()), PeerHost::Client);
    assert_eq!(unrelated(u32::MAX - 7), PeerHost::Client);
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn dux_pid_is_read_from_the_lockfile() {
    let dir = tempdir().unwrap();
    let paths = DuxPaths {
        config_path: dir.path().join("config.toml"),
        sessions_db_path: dir.path().join("sessions.sqlite3"),
        worktrees_root: dir.path().join("worktrees"),
        lock_path: dir.path().join("dux.lock"),
        root: dir.path().to_path_buf(),
    };
    assert_eq!(dux_tui_pid(&paths), None);
    fs::write(&paths.lock_path, "4321\n").unwrap();
    assert_eq!(dux_tui_pid(&paths), Some(4321));
    fs::write(&paths.lock_path, "").unwrap();
    assert_eq!(dux_tui_pid(&paths), None);
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
fn target_resolution_matches_directory_title_and_id_aliases() {
    let dir = tempdir().unwrap();
    let worktree = dir.path().join("Feature Login");
    let mut s = session("s1", "claude", "renamed", &worktree);
    s.title = Some("My Title".to_string());
    let sessions = [s];
    for alias in ["Feature Login", "feature-login", "My Title", "s1"] {
        assert_eq!(resolve_target(alias, &sessions).unwrap().handle, "renamed");
    }
}

#[test]
fn target_resolution_rejects_ambiguous_aliases() {
    let dir = tempdir().unwrap();
    let a = session("s1", "claude", "one", &dir.path().join("x/same"));
    let b = session("s2", "codex", "two", &dir.path().join("y/same"));
    let err = resolve_target("same", &[a, b]).unwrap_err().to_string();
    assert!(err.contains("ambiguous peer target"), "{err}");
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
fn routable_sessions_exclude_exited_and_missing_worktrees() {
    let dir = tempdir().unwrap();
    let live_wt = dir.path().join("live");
    fs::create_dir_all(&live_wt).unwrap();
    let mut exited = session("s2", "codex", "exited", &live_wt);
    exited.exited = true;
    let missing = session("s3", "claude", "missing", &dir.path().join("missing"));
    let mut deleted = session("s4", "claude", "deleted", &live_wt);
    deleted.deleted = true;

    assert!(session("s1", "claude", "live", &live_wt).is_routable());
    assert!(!exited.is_routable());
    assert!(!missing.is_routable());
    assert!(!deleted.is_routable());
}

#[test]
fn explicit_claude_peers_refuses_codex_target() {
    let dir = tempdir().unwrap();
    let sender = SenderContext {
        handle: "sender".to_string(),
        session: Some(session(
            "s1",
            "claude",
            "sender",
            &dir.path().join("sender"),
        )),
    };
    let target = PeerTarget {
        handle: "target".to_string(),
        session: Some(session("s2", "codex", "target", &dir.path().join("target"))),
    };

    let err = choose_transport(TransportPreference::ClaudePeers, &sender, &target)
        .unwrap_err()
        .to_string();

    assert!(err.contains("cannot target provider codex"));
}

#[test]
fn explicit_amq_is_honoured_for_a_claude_target() {
    let dir = tempdir().unwrap();
    let sender = SenderContext {
        handle: "sender".to_string(),
        session: None,
    };
    let target = PeerTarget {
        handle: "target".to_string(),
        session: Some(session("s2", "claude", "target", dir.path())),
    };
    assert_eq!(
        choose_transport(TransportPreference::Amq, &sender, &target).unwrap(),
        ChosenTransport::Amq
    );
}

#[test]
fn auto_routes_a_non_claude_target_via_amq() {
    let dir = tempdir().unwrap();
    let sender = SenderContext {
        handle: "sender".to_string(),
        session: Some(session("s1", "claude", "sender", dir.path())),
    };
    let target = PeerTarget {
        handle: "target".to_string(),
        session: Some(session("s2", "codex", "target", dir.path())),
    };
    assert_eq!(
        choose_transport(TransportPreference::Auto, &sender, &target).unwrap(),
        ChosenTransport::Amq
    );
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

/// Default Claude targets go to Claude Peers whatever the sender's provider
/// (fork commit "Default Claude targets to Claude Peers").
#[test]
fn worktree_endpoints_still_prefer_claude_peers() {
    let dir = tempdir().unwrap();
    let sender = SenderContext {
        handle: "sender".to_string(),
        session: Some(session("s1", "codex", "sender", &dir.path().join("sender"))),
    };
    let target = PeerTarget {
        handle: "target".to_string(),
        session: Some(session(
            "s2",
            "claude",
            "target",
            &dir.path().join("target"),
        )),
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
    alias.branch = Some("stable-handle".to_string());
    let exact = session("s2", "claude", "stable-handle", dir.path());

    let sender = infer_sender(Some("stable-handle"), &[alias, exact]).unwrap();

    assert_eq!(sender.handle, "stable-handle");
    assert_eq!(sender.session.unwrap().id, "s2");
}

#[test]
fn explicit_from_that_sanitises_to_nothing_is_rejected() {
    let err = infer_sender(Some("!!!"), &[]).unwrap_err().to_string();
    assert!(err.contains("empty AMQ handle"));
}

#[test]
fn non_claude_sender_gets_dux_reply_hint_for_claude_peers() {
    let dir = tempdir().unwrap();
    let sender = SenderContext {
        handle: "sender".to_string(),
        session: Some(session("s1", "codex", "sender", &dir.path().join("sender"))),
    };

    let (from_id, message) = claude_peers_sender(&sender, &[], "status?", None);

    assert_eq!(from_id, "sender");
    assert!(message.contains("dux peer send sender"));
    assert!(message.ends_with("status?"));
}

#[test]
fn registered_claude_sender_sends_as_itself_without_preamble() {
    let dir = tempdir().unwrap();
    let wt = dir.path().join("sender");
    fs::create_dir_all(&wt).unwrap();
    let sender = SenderContext {
        handle: "sender".to_string(),
        session: Some(session("s1", "claude", "sender", &wt)),
    };
    let mut registration = peer("peer-7", &wt, 1, "2026-09-16T00:00:00Z");
    registration.pid = None;

    let (from_id, message) = claude_peers_sender(&sender, &[registration], "status?", None);

    assert_eq!(from_id, "peer-7");
    assert_eq!(message, "status?");
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[test]
fn send_arg_parser_allows_message_words_that_look_like_flags() {
    let parsed = parse_send_args(&args(&[
        "--transport",
        "amq",
        "worker",
        "--please",
        "respond",
    ]))
    .unwrap()
    .unwrap();

    assert_eq!(parsed.target, "worker");
    assert_eq!(parsed.message, "--please respond");
    assert_eq!(parsed.transport, TransportPreference::Amq);
}

/// `dux peer send` keeps the fork's exact CLI; the orchestrator prompt
/// tells agents to call it with these arguments.
#[test]
fn send_arg_parser_accepts_the_fork_cli_shape() {
    let parsed = parse_send_args(&args(&[
        "--from",
        "Me",
        "--transport",
        "claude-peers",
        "worker",
        "multi",
        "word",
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(parsed.from.as_deref(), Some("Me"));
    assert_eq!(parsed.transport, TransportPreference::ClaudePeers);
    assert_eq!(parsed.target, "worker");
    assert_eq!(parsed.message, "multi word");

    let dashed = parse_send_args(&args(&["--", "--odd-target", "hi"]))
        .unwrap()
        .unwrap();
    assert_eq!(dashed.target, "--odd-target");
    assert_eq!(dashed.transport, TransportPreference::Auto);

    assert!(parse_send_args(&args(&["--help"])).unwrap().is_none());
    for bad in [
        &["worker"][..],
        &["--from"][..],
        &["--transport", "carrier-pigeon", "w", "m"][..],
        &["--bogus", "w", "m"][..],
    ] {
        assert!(parse_send_args(&args(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn peer_subcommands_reject_unknown_flags_and_subcommands() {
    let dir = tempdir().unwrap();
    let paths = DuxPaths {
        config_path: dir.path().join("config.toml"),
        sessions_db_path: dir.path().join("sessions.sqlite3"),
        worktrees_root: dir.path().join("worktrees"),
        lock_path: dir.path().join("dux.lock"),
        root: dir.path().to_path_buf(),
    };
    assert!(run_peer(&args(&["bogus"]), &paths).is_err());
    assert!(run_peer(&args(&["list", "--nope"]), &paths).is_err());
    assert!(run_peer(&args(&["sync-amq", "--nope"]), &paths).is_err());
    assert!(run_peer(&args(&["--help"]), &paths).is_ok());
}

#[test]
fn broker_response_parsing_checks_status_and_json() {
    assert_eq!(
        parse_broker_response("HTTP/1.1 200 OK\r\nX: y\r\n\r\n{\"ok\":true}").unwrap(),
        json!({ "ok": true })
    );
    let err = parse_broker_response("HTTP/1.1 500 Oops\r\n\r\n\u{1b}]0;x\u{7}boom")
        .unwrap_err()
        .to_string();
    assert!(err.contains("HTTP 500"));
    assert!(!err.contains('\u{1b}'), "broker body must be sanitised");
    assert!(parse_broker_response("garbage").is_err());
    assert!(parse_broker_response("HTTP/1.1 200 OK\r\n\r\nnot json").is_err());
}
