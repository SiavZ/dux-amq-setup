//! Switching the background server on from the terminal UI, seen from a browser.
//!
//! The terminal UI claims every running pty the moment a start somebody at its
//! keyboard asked for comes up. This drives the web half of that for real: a
//! real engine with a running agent and a running terminal, a real
//! `BackgroundServer` on a loopback port serviced the way the terminal UI's run
//! loop services it, the claim made through the same `dux-core` call the
//! terminal UI makes and announced through the same seam, and a real browser on
//! the events socket and the agent's pty socket.
//!
//! What is not real is the terminal UI itself: `dux-tui` cannot be linked into a
//! `dux-web` test (the web layer never sees the terminal UI), so its two lines
//! around the claim are stood in for here. Its own tests cover which starts
//! claim and which do not.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use dux_core::background_serve::TUI_DEVICE_LABEL;
use dux_core::config::{DuxPaths, ProjectConfig};
use dux_core::engine::Engine;
use dux_core::ids::{TabId, TabIdRef};
use dux_core::storage::SessionStore;
use dux_web::background::BackgroundServer;
use dux_web::bootstrap::bootstrap_engine;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

type ClientWs =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn sample_session(id: &str, worktree: &str) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        slot_tab_id: format!("{id}-slot"),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: true,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Active,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: "p1".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: "feat".to_string(),
            initial_branch: "feat".to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: worktree.to_string(),
        }),
    }
}

/// An engine with agent `s1` running (`cat`, so typing echoes) and a standalone
/// terminal running, in the state the terminal UI leaves it in before serving:
/// its global workers already up.
fn engine_with_an_agent_and_a_terminal_running() -> (Engine, String, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    {
        let store = SessionStore::open(&paths.sessions_db_path).unwrap();
        store
            .upsert_project(&ProjectConfig {
                id: "p1".to_string(),
                path: root.to_string_lossy().into_owned(),
                name: Some("p1-name".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();
        store
            .create_session(&sample_session("s1", root.to_string_lossy().as_ref()))
            .unwrap();
    }
    let mut engine = bootstrap_engine(&paths).unwrap();
    engine
        .changed_files_poller_started
        .store(true, std::sync::atomic::Ordering::Relaxed);
    engine.providers.insert(
        TabId::new("s1-slot"),
        dux_core::pty::PtyClient::spawn("cat", &[], &root, 24, 80, 1000).expect("spawn cat"),
    );
    engine.config.terminal.command = "cat".to_string();
    engine.config.terminal.args = vec![];
    let (terminal, _) = engine
        .create_standalone_terminal(24, 80)
        .expect("standalone terminal");
    (engine, terminal, tmp)
}

/// What the browser thread asks the thread holding the engine to do.
enum Ask {
    /// The browser is listening: switch serving on here and claim.
    ClaimNow,
    /// Type `bytes` into the agent from the terminal UI's seat.
    TuiTypes(&'static [u8]),
}

async fn next_event_frame(
    ws: &mut ClientWs,
    event: &str,
    within: Duration,
) -> Option<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
            && v["event"].as_str() == Some(event)
        {
            return Some(v);
        }
    }
    None
}

async fn accumulate_until(ws: &mut ClientWs, needle: &str, within: Duration) -> String {
    let mut acc = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(m))) = tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            if let Message::Binary(b) = m {
                acc.extend_from_slice(&b);
            }
            if String::from_utf8_lossy(&acc).contains(needle) {
                break;
            }
        }
    }
    String::from_utf8_lossy(&acc).into_owned()
}

/// The browser's half of the journey, run on its own thread and runtime while
/// the test thread services the engine the way the terminal UI's run loop does.
async fn browser(
    addr: std::net::SocketAddr,
    terminal: String,
    tui_conn: u64,
    ask: mpsc::Sender<Ask>,
) {
    let (mut events, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/events"))
        .await
        .expect("connect the events socket");
    next_event_frame(&mut events, "connected", Duration::from_secs(8))
        .await
        .expect("the events handshake");
    events
        .send(Message::Text(r#"{"subscribe":["sessions"]}"#.into()))
        .await
        .unwrap();
    // Let the subscribe land before anything is announced.
    tokio::time::sleep(Duration::from_millis(300)).await;

    ask.send(Ask::ClaimNow).unwrap();

    // Each claim reaches the browser as the `pty.owner` a launch's claim sends,
    // naming the terminal UI.
    let mut owned = std::collections::BTreeMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while owned.len() < 2 && tokio::time::Instant::now() < deadline {
        if let Some(frame) =
            next_event_frame(&mut events, "pty.owner", Duration::from_secs(1)).await
        {
            owned.insert(
                frame["id"].as_str().unwrap_or_default().to_string(),
                (frame["owner"].clone(), frame["device"].clone()),
            );
        }
    }
    let expected = (
        serde_json::Value::String(tui_conn.to_string()),
        serde_json::Value::String(TUI_DEVICE_LABEL.to_string()),
    );
    assert_eq!(owned.get("s1-slot"), Some(&expected), "{owned:?}");
    assert_eq!(owned.get(&terminal), Some(&expected), "{owned:?}");

    // Opening the agent: the handshake says the terminal UI is driving it.
    let url = format!("ws://{addr}/ws/sessions/s1/pty");
    let (mut pty, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect the agent's pty socket");
    let hello = next_event_frame(&mut pty, "connected", Duration::from_secs(8))
        .await
        .expect("the pty handshake");
    assert_eq!(hello["owner"].as_str(), Some(tui_conn.to_string().as_str()));
    assert_eq!(hello["owner_device"].as_str(), Some(TUI_DEVICE_LABEL));

    // The terminal UI keeps typing into it, and the browser watches it arrive.
    ask.send(Ask::TuiTypes(b"dux-tui-still-types\n")).unwrap();
    let seen = accumulate_until(&mut pty, "dux-tui-still-types", Duration::from_secs(8)).await;
    assert!(seen.contains("dux-tui-still-types"), "{seen:?}");

    // A plain attach resize and the browser's typing take nothing.
    pty.send(Message::Text(r#"{"rows":30,"cols":100}"#.into()))
        .await
        .unwrap();
    pty.send(Message::Binary(
        b"dux-plain-attach-marker\n".to_vec().into(),
    ))
    .await
    .unwrap();
    let stolen =
        accumulate_until(&mut pty, "dux-plain-attach-marker", Duration::from_secs(2)).await;
    assert!(
        !stolen.contains("dux-plain-attach-marker"),
        "a plain attach must not take a pty the terminal UI claimed"
    );

    // Take over does.
    pty.send(Message::Text(
        r#"{"rows":30,"cols":100,"takeover":true}"#.into(),
    ))
    .await
    .unwrap();
    pty.send(Message::Binary(b"dux-took-over-marker\n".to_vec().into()))
        .await
        .unwrap();
    let after = accumulate_until(&mut pty, "dux-took-over-marker", Duration::from_secs(8)).await;
    assert!(
        after.contains("dux-took-over-marker"),
        "a flagged take-over must hand the browser the pty"
    );
}

/// THE JOURNEY. The user has an agent and a terminal running in the terminal
/// UI and switches serving on. A browser that opens is told the terminal UI is
/// driving both, sees the terminal UI's typing arrive, cannot take the agent by
/// merely attaching, and can by pressing Take over.
#[test]
fn a_browser_finds_everything_running_driven_by_the_tui_until_it_takes_over() {
    let (mut engine, terminal, _tmp) = engine_with_an_agent_and_a_terminal_running();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let addr = listener.local_addr().expect("bound address");
    let mut server =
        BackgroundServer::start(&mut engine, vec![listener], vec![format!("http://{addr}")])
            .expect("the serve starts");
    let seat = server.ownership();

    let (ask_tx, ask_rx) = mpsc::channel();
    let browser_terminal = terminal.clone();
    let tui_conn = seat.conn_id;
    let browser_thread = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("browser runtime")
            .block_on(browser(addr, browser_terminal, tui_conn, ask_tx));
    });

    // The terminal UI's run loop: service the serve every iteration, and act on
    // what the journey asks for in between.
    let deadline = Instant::now() + Duration::from_secs(60);
    while !browser_thread.is_finished() {
        assert!(
            Instant::now() < deadline,
            "the journey did not finish in time"
        );
        server.service(&mut engine);
        match ask_rx.try_recv() {
            Ok(Ask::ClaimNow) => {
                // What the terminal UI does in the step that brings a serve it
                // was asked for up: claim everything running, announce it.
                let claimed = seat.claim_every_running_pty(&engine);
                assert_eq!(claimed.len(), 2, "the agent and the terminal: {claimed:?}");
                server.publish_ownership_events(&claimed);
            }
            Ok(Ask::TuiTypes(bytes)) => {
                let client = engine
                    .providers
                    .get(TabIdRef::new("s1-slot"))
                    .expect("the agent is running");
                assert!(
                    seat.owners.write_if_owner("s1-slot", seat.conn_id, || {
                        client.enqueue_bytes(bytes);
                    }),
                    "the terminal UI still drives its agent"
                );
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let result = browser_thread.join();
    let still_mine = seat.owners.is_owner("s1-slot", seat.conn_id);
    let terminal_mine = seat.owners.is_owner(&terminal, seat.conn_id);
    server.stop();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert!(!still_mine, "the take-over moved the agent to the browser");
    assert!(
        terminal_mine,
        "and left the terminal it never touched with the TUI"
    );
}
