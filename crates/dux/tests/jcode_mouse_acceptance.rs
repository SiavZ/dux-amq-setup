//! Opt-in acceptance check (fork cdb39e2f): the REAL dux binary drives a REAL
//! jcode replay, and the mouse wheel over the jcode pane must move jcode's own
//! transcript, in interactive fullscreen and in the windowed view.
//!
//! Replay is offline: no model request is sent. All dux state is isolated in a
//! temporary `DUX_HOME` under the artifacts directory.
//!
//! Environment:
//! - `DUX_ACCEPTANCE_JCODE`: absolute path of the jcode executable.
//! - `DUX_ACCEPTANCE_SESSION`: a saved jcode session JSON with enough transcript
//!   to scroll (for example one under `~/.jcode/sessions`).
//! - `DUX_ACCEPTANCE_ARTIFACTS`: writable directory for terminal snapshots. They
//!   contain session text, so point it somewhere private.
//! - `DUX_ACCEPTANCE_DUX` (optional): the dux binary; defaults to this build.
//! - `DUX_ACCEPTANCE_FORWARD=false` (optional): pins `forward_scroll = false` as
//!   a negative control, which must reproduce the dead wheel.
//!
//! Run: `cargo test -p dux --test jcode_mouse_acceptance -- --ignored --nocapture`.

use std::{fs, path::Path, process::Command, thread, time::Duration};

use dux_core::config::Config;
use dux_core::model::{AgentSession, AgentWorkspace, FolderWorkspace, ProviderKind, SessionStatus};
use dux_core::pty::PtyClient;
use dux_core::storage::SessionStore;

fn screen(pty: &PtyClient) -> String {
    let snapshot = pty.snapshot();
    let mut lines = vec![String::new(); snapshot.rows as usize];
    for cell in snapshot.cells {
        lines[cell.row as usize].push_str(&cell.symbol);
    }
    lines.join("\n")
}

fn wait_for(pty: &PtyClient, needle: &str, secs: u64) -> bool {
    for _ in 0..secs * 10 {
        if screen(pty).contains(needle) {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
#[ignore = "requires an installed jcode and a saved session for offline replay"]
fn real_jcode_mouse_scroll_in_dux() {
    let session = std::env::var("DUX_ACCEPTANCE_SESSION").expect("saved session path");
    let jcode = std::env::var("DUX_ACCEPTANCE_JCODE").expect("jcode binary path");
    let artifacts = std::env::var("DUX_ACCEPTANCE_ARTIFACTS").expect("artifact directory");
    let dux = std::env::var("DUX_ACCEPTANCE_DUX")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_dux").to_string());
    let artifacts = Path::new(&artifacts);
    fs::create_dir_all(artifacts).unwrap();
    let root = tempfile::tempdir_in(artifacts).unwrap();
    let folder = root.path().join("work");
    fs::create_dir(&folder).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&folder)
            .status()
            .unwrap()
            .success()
    );
    let replay = root.path().join("replay.json");
    fs::copy(session, &replay).unwrap();
    let home = root.path().join("dux");
    fs::create_dir(&home).unwrap();

    let mut cfg = Config::default();
    cfg.providers.ensure_defaults();
    cfg.ui.github_integration = false;
    // A fresh DUX_HOME is a first launch: keep the welcome and what's-new
    // screens from taking the first keypresses.
    cfg.ui.disable_automated_welcome_screen = true;
    cfg.ui.disable_release_notes = true;
    let provider = cfg.providers.commands.get_mut("jcode").unwrap();
    provider.command = jcode;
    provider.args = vec![
        "--no-update".into(),
        "--no-selfdev".into(),
        "replay".into(),
        replay.to_string_lossy().into(),
        "--speed".into(),
        "10000000".into(),
        "--auto-edit".into(),
    ];
    provider.resume_args = None;
    provider.resume_by_id_args = None;
    let forwarding = std::env::var("DUX_ACCEPTANCE_FORWARD").as_deref() != Ok("false");
    // Forwarding runs on the SHIPPED jcode policy (Some(true)); the negative
    // control pins it off. Drags stay in dux either way, as on the machine
    // where the dead wheel was reported.
    if !forwarding {
        provider.forward_scroll = Some(false);
    }
    provider.forward_mouse = Some(false);
    fs::write(home.join("config.toml"), toml::to_string(&cfg).unwrap()).unwrap();

    let store = SessionStore::open(&home.join("sessions.sqlite3")).unwrap();
    let now = chrono::Utc::now();
    store
        .create_session(&AgentSession {
            id: "acceptance-session".into(),
            agent_handle: "acceptance-session".into(),
            shared_workspace: false,
            deleted_at: None,
            slot_tab_id: "acceptance-session-slot".into(),
            provider: ProviderKind::from_str("jcode"),
            workspace: AgentWorkspace::Folder(FolderWorkspace {
                folder_path: folder.to_string_lossy().into(),
            }),
            title: Some("scroll acceptance".into()),
            started_providers: vec![],
            desired_running: false,
            auto_reopen_enabled: false,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        })
        .unwrap();
    drop(store);

    let pty = PtyClient::spawn(
        "/usr/bin/env",
        &[format!("DUX_HOME={}", home.display()), dux],
        &folder,
        45,
        180,
        200,
    )
    .unwrap();
    assert!(
        wait_for(&pty, "scroll acceptance", 30),
        "dux never listed the seeded agent:\n{}",
        screen(&pty)
    );
    fs::write(artifacts.join("01-start.txt"), screen(&pty)).unwrap();
    // Select the agent row (the Inactive group header is row 0) and launch it
    // into interactive fullscreen.
    pty.write_bytes(b"j").unwrap();
    thread::sleep(Duration::from_millis(500));
    pty.write_bytes(b"\r").unwrap();
    thread::sleep(Duration::from_secs(15));
    // Freeze the replay first, or automatic playback could masquerade as wheel
    // movement. Compare only transcript cells, excluding clocks and spinners.
    pty.write_bytes(b" ").unwrap();
    thread::sleep(Duration::from_secs(2));
    fs::write(artifacts.join("02-launched.txt"), screen(&pty)).unwrap();
    assert!(
        screen(&pty).contains("Jcode agent") || screen(&pty).contains("jcode"),
        "must open the real jcode pane:\n{}",
        screen(&pty)
    );
    let crop = |left: u16| {
        pty.snapshot()
            .cells
            .into_iter()
            .filter(|c| c.row >= 5 && c.row < 22 && c.col >= left && c.col < left + 50)
            .map(|c| c.symbol.to_string())
            .collect::<String>()
    };
    let wheel = |up: bool, x: u16| {
        for _ in 0..30 {
            pty.write_bytes(format!("\x1b[<{};{};15M", if up { 64 } else { 65 }, x).as_bytes())
                .unwrap();
            thread::sleep(Duration::from_millis(35));
        }
        thread::sleep(Duration::from_secs(2));
    };
    let check_mode = |label: &str, left: u16, x: u16| {
        let baseline = crop(left);
        thread::sleep(Duration::from_secs(2));
        assert_eq!(
            baseline,
            crop(left),
            "{label}: paused transcript must stay stable without input"
        );
        fs::write(artifacts.join(format!("{label}-before.txt")), screen(&pty)).unwrap();
        wheel(true, x);
        let up = crop(left);
        fs::write(artifacts.join(format!("{label}-up.txt")), screen(&pty)).unwrap();
        wheel(false, x);
        let down = crop(left);
        fs::write(artifacts.join(format!("{label}-down.txt")), screen(&pty)).unwrap();
        if forwarding {
            assert_ne!(baseline, up, "{label}: wheel up must move the transcript");
            assert_ne!(up, down, "{label}: wheel down must move the transcript");
        } else {
            assert_eq!(baseline, up, "{label}: disabled forwarding must be dead");
            assert_eq!(up, down, "{label}: disabled forwarding must be dead");
        }
        println!(
            "{label}: forwarding={forwarding}, idle_stable=true, up_changed={}, down_changed={}",
            baseline != up,
            up != down
        );
    };
    check_mode("interactive", 10, 55);
    // Ctrl-g minimizes back to the windowed view; the wheel must still reach
    // jcode there, with no keyboard focus on the pane.
    pty.write_bytes(b"\x07").unwrap();
    thread::sleep(Duration::from_secs(3));
    check_mode("view", 42, 75);
    // Close only this isolated dux and its replay child.
    pty.write_bytes(b"\x03").unwrap();
    thread::sleep(Duration::from_secs(1));
}
