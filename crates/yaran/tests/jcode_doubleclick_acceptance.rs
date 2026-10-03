//! Opt-in repro: a double-click on a jcode agent row must open it the same way
//! as any other harness. Real yaran binary, real jcode (offline replay, no model
//! call), isolated YARAN_HOME and AMQ root under the artifacts directory.
//!
//! Environment: YARAN_ACCEPTANCE_JCODE, YARAN_ACCEPTANCE_SESSION,
//! YARAN_ACCEPTANCE_ARTIFACTS, optional YARAN_ACCEPTANCE_YARAN (see
//! jcode_mouse_acceptance.rs).
//! Run: `cargo test -p yaran --test jcode_doubleclick_acceptance -- --ignored --nocapture`.

use std::{fs, path::Path, process::Command, thread, time::Duration};

use yaran_core::config::Config;
use yaran_core::model::{
    AgentSession, AgentWorkspace, FolderWorkspace, ProviderKind, SessionStatus,
};
use yaran_core::pty::PtyClient;
use yaran_core::storage::SessionStore;

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

/// Screen row (0-based) whose text contains `needle`.
fn row_of(pty: &PtyClient, needle: &str) -> Option<u16> {
    screen(pty)
        .lines()
        .position(|l| l.contains(needle))
        .map(|r| r as u16)
}

fn session(id: &str, title: &str, provider: &str, folder: &Path) -> AgentSession {
    let now = chrono::Utc::now();
    AgentSession {
        id: id.into(),
        agent_handle: id.into(),
        shared_workspace: false,
        deleted_at: None,
        slot_tab_id: format!("{id}-slot"),
        provider: ProviderKind::from_str(provider),
        workspace: AgentWorkspace::Folder(FolderWorkspace {
            folder_path: folder.to_string_lossy().into(),
        }),
        title: Some(title.into()),
        started_providers: vec![],
        desired_running: false,
        auto_reopen_enabled: false,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
    }
}

#[test]
#[ignore = "requires an installed jcode and a saved session for offline replay"]
fn double_click_opens_jcode_like_any_other_harness() {
    let session_json = std::env::var("YARAN_ACCEPTANCE_SESSION").expect("saved session path");
    let jcode = std::env::var("YARAN_ACCEPTANCE_JCODE").expect("jcode binary path");
    let artifacts = std::env::var("YARAN_ACCEPTANCE_ARTIFACTS").expect("artifact directory");
    let yaran = std::env::var("YARAN_ACCEPTANCE_YARAN")
        .unwrap_or_else(|_| env!("CARGO_BIN_EXE_yaran").to_string());
    let artifacts = Path::new(&artifacts);
    fs::create_dir_all(artifacts).unwrap();
    let root = tempfile::tempdir_in(artifacts).unwrap();
    let home = root.path().join("home");
    let amq = root.path().join("amq");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&amq).unwrap();
    let mk = |name: &str| {
        let d = root.path().join(name);
        fs::create_dir(&d).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q", "-b", "main"])
                .current_dir(&d)
                .status()
                .unwrap()
                .success()
        );
        d
    };
    let jfolder = mk("jwork");
    let sfolder = mk("swork");
    let replay = root.path().join("replay.json");
    fs::copy(session_json, &replay).unwrap();

    let mut cfg = Config::default();
    cfg.providers.ensure_defaults();
    cfg.ui.github_integration = false;
    cfg.ui.disable_automated_welcome_screen = true;
    cfg.ui.disable_release_notes = true;
    let provider = cfg.providers.commands.get_mut("jcode").unwrap();
    // A LIVE jcode (not a replay) at its input box, the state a real agent is
    // in when you double-click it. Its own JCODE_HOME keeps it off the user's
    // sessions and credentials; with no login it never calls a model.
    // YARAN_ACCEPTANCE_STANDIN swaps in a stand-in that turns on the same
    // terminal modes jcode does and prints every byte it receives.
    let jhome = root.path().join("jcode-home");
    fs::create_dir_all(&jhome).unwrap();
    let _ = &replay;
    if let Ok(standin) = std::env::var("YARAN_ACCEPTANCE_STANDIN") {
        provider.command = standin;
        provider.args = vec![];
    } else {
        provider.command = "/usr/bin/env".into();
        provider.args = vec![
            format!("JCODE_HOME={}", jhome.display()),
            "JCODE_NON_INTERACTIVE=".into(),
            jcode.clone(),
            "--no-update".into(),
            "--no-selfdev".into(),
        ];
    }
    provider.resume_args = None;
    provider.resume_by_id_args = None;
    // As the user's config ships jcode: plain left presses stay with yaran.
    provider.forward_mouse = Some(false);
    // The comparison harness: a plain line-mode child, the way claude/codex
    // behave from yaran's side before they draw.
    let shell = cfg.providers.commands.get_mut("codex").unwrap();
    shell.command = "sh".into();
    shell.args = vec!["-c".into(), "echo SHELL-AGENT-READY; exec cat".into()];
    shell.resume_args = None;
    shell.resume_by_id_args = None;
    fs::write(home.join("config.toml"), toml::to_string(&cfg).unwrap()).unwrap();

    let store = SessionStore::open(&home.join("sessions.sqlite3")).unwrap();
    store
        .create_session(&session("jc", "jc-agent", "jcode", &jfolder))
        .unwrap();
    store
        .create_session(&session("sh", "sh-agent", "codex", &sfolder))
        .unwrap();
    drop(store);

    let pty = PtyClient::spawn(
        "/usr/bin/env",
        &[
            format!("YARAN_HOME={}", home.display()),
            format!("AMQ_GLOBAL_ROOT={}", amq.display()),
            format!("AM_ROOT={}", amq.display()),
            yaran,
        ],
        &jfolder,
        45,
        180,
        200,
    )
    .unwrap();
    assert!(
        wait_for(&pty, "jc-agent", 30),
        "yaran never listed the agents:\n{}",
        screen(&pty)
    );

    let double_click = |label: &str| {
        let row =
            row_of(&pty, label).unwrap_or_else(|| panic!("no row for {label}:\n{}", screen(&pty)));
        let col = 6u16;
        for _ in 0..2 {
            pty.write_bytes(format!("\x1b[<0;{};{}M", col + 1, row + 1).as_bytes())
                .unwrap();
            pty.write_bytes(format!("\x1b[<0;{};{}m", col + 1, row + 1).as_bytes())
                .unwrap();
            thread::sleep(Duration::from_millis(60));
        }
    };

    // Shell agent first: the reference behaviour.
    double_click("sh-agent");
    assert!(
        wait_for(&pty, "SHELL-AGENT-READY", 15),
        "shell agent never started:\n{}",
        screen(&pty)
    );
    thread::sleep(Duration::from_secs(2));
    let shell_screen = screen(&pty);
    fs::write(artifacts.join("dc-shell.txt"), &shell_screen).unwrap();
    pty.write_bytes(b"typed-into-shell\r").unwrap();
    let shell_typed = wait_for(&pty, "typed-into-shell", 5);
    fs::write(artifacts.join("dc-shell-typed.txt"), screen(&pty)).unwrap();
    // Leave the pane the same way a user would before clicking another row.
    pty.write_bytes(b"\x1b").unwrap();
    thread::sleep(Duration::from_millis(500));

    double_click("jc-agent");
    thread::sleep(Duration::from_secs(15));
    let jcode_screen = screen(&pty);
    fs::write(artifacts.join("dc-jcode.txt"), &jcode_screen).unwrap();
    let log = fs::read_to_string(home.join("yaran.log")).unwrap_or_default();
    fs::write(artifacts.join("dc-yaran.log"), &log).unwrap();

    println!("shell typed reached pane: {shell_typed}");
    println!(
        "--- shell header ---\n{}",
        shell_screen.lines().take(3).collect::<Vec<_>>().join("\n")
    );
    println!(
        "--- jcode screen ---\n{}",
        jcode_screen.lines().take(45).collect::<Vec<_>>().join("\n")
    );

    // Both agents are now RUNNING. The everyday gesture: select another row,
    // then double-click back onto a live agent, and type. Compare the result
    // for the shell harness and for jcode.
    let focus_state = |pty: &PtyClient| {
        let s = screen(pty);
        let footer = s.lines().rev().nth(1).unwrap_or("").to_string();
        let header = s.lines().nth(1).unwrap_or("").to_string();
        (header, footer)
    };
    for (label, typed) in [
        ("sh-agent", "second-into-shell"),
        ("jc-agent", "second-into-jcode"),
    ] {
        // Park the selection on the other row first (single click), like a user.
        let other = if label == "sh-agent" {
            "jc-agent"
        } else {
            "sh-agent"
        };
        let row = row_of(&pty, other).unwrap();
        pty.write_bytes(format!("\x1b[<0;7;{}M\x1b[<0;7;{}m", row + 1, row + 1).as_bytes())
            .unwrap();
        thread::sleep(Duration::from_millis(700));
        double_click(label);
        thread::sleep(Duration::from_secs(2));
        let (header, footer) = focus_state(&pty);
        fs::write(artifacts.join(format!("live-{label}.txt")), screen(&pty)).unwrap();
        pty.write_bytes(typed.as_bytes()).unwrap();
        thread::sleep(Duration::from_secs(2));
        let reached = screen(&pty).contains(typed);
        fs::write(
            artifacts.join(format!("live-{label}-typed.txt")),
            screen(&pty),
        )
        .unwrap();
        println!(
            "LIVE {label}: typed_reached={reached}\n  header={}\n  footer={}",
            header.trim_end(),
            footer.trim_end()
        );
        // Clear whatever was typed, then leave the pane.
        pty.write_bytes(b"\x15").unwrap();
        pty.write_bytes(b"\x1b").unwrap();
        thread::sleep(Duration::from_millis(500));
    }
    // The gesture itself: select each agent, then double-click INSIDE its pane.
    let mut opened = Vec::new();
    for label in ["sh-agent", "jc-agent"] {
        let row = row_of(&pty, label).unwrap();
        pty.write_bytes(format!("\x1b[<0;7;{}M\x1b[<0;7;{}m", row + 1, row + 1).as_bytes())
            .unwrap();
        thread::sleep(Duration::from_millis(800));
        let fullscreen = pane_double_click(&pty);
        fs::write(artifacts.join(format!("pane-dc-{label}.txt")), screen(&pty)).unwrap();
        println!("PANE-DC {label}: opened_fullscreen={fullscreen}");
        opened.push(fullscreen);
        // Back out of fullscreen / interactive before the next agent.
        pty.write_bytes(b"\x07").unwrap();
        thread::sleep(Duration::from_millis(500));
        pty.write_bytes(b"\x1b").unwrap();
        thread::sleep(Duration::from_millis(500));
    }
    pty.write_bytes(b"\x03").unwrap();
    thread::sleep(Duration::from_secs(1));
    assert!(
        opened[0],
        "a double click must open the plain harness fullscreen"
    );
    assert!(
        opened[1],
        "a double click must open jcode fullscreen exactly like the other harness"
    );
}

/// Where a double-click INSIDE the agent pane lands: yaran's fullscreen
/// interactive view ("Fullscreen. Keys go to the agent verbatim") or not.
fn pane_double_click(pty: &PtyClient) -> bool {
    // Middle of the agent pane: column 80, row 20 (1-based SGR).
    for _ in 0..2 {
        pty.write_bytes(b"\x1b[<0;80;20M\x1b[<0;80;20m").unwrap();
        thread::sleep(Duration::from_millis(60));
    }
    thread::sleep(Duration::from_secs(2));
    // The fullscreen overlay's own footer: "<Ctrl-g> minimize".
    screen(pty).contains("> minimize")
}
