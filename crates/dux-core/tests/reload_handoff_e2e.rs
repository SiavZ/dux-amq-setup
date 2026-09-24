//! The reload handoff, end to end, across a real `exec`.
//!
//! The unit tests for `dux_core::pty_reattach` and `dux_core::reload_handoff`
//! each check their own half: that a descriptor can be adopted, and that a
//! manifest survives a round trip through a file. Neither can check the thing
//! the feature actually promises, because that only happens when a process
//! really replaces itself and a DIFFERENT program image picks the agent back up.
//!
//! That is what this does. The test starts an agent on a PTY, writes a
//! handoff, and `exec`s. The second image reads the handoff, adopts the
//! inherited descriptor, and talks to the agent the first image started. If any
//! link is wrong (the descriptor closed by the exec, the manifest lost, the
//! adopted fd pointing elsewhere) the conversation fails and so does this test.
//!
//! # Why this target has no test harness
//!
//! The process that execs has to run OUR code on the other side, not the
//! libtest harness, which would just run the suite again. So this target is
//! declared `harness = false` in `Cargo.toml` and `main` dispatches on an
//! environment variable: unset, it is the test runner; set, it plays one
//! generation of the reload. This used to be a separate `[[bin]]` of dux-core,
//! which meant `cargo install` and every release build shipped a test helper to
//! users. Folding it in here means it only exists when tests are built.

use std::path::{Path, PathBuf};
use std::process::Command;

use dux_core::pty::PtyClient;
use dux_core::reload_handoff::Handoff;

const ROLE_VAR: &str = "DUX_RELOAD_ROLE";
const HANDOFF_VAR: &str = "DUX_RELOAD_HANDOFF";
const AMBIENT_VAR: &str = "DUX_RELOAD_AMBIENT_FDS";
const LOCK_VAR: &str = "DUX_RELOAD_LOCK";

type Case = (&'static str, fn());

const CASES: &[Case] = &[
    (
        "an_agent_survives_a_reload_and_answers_the_next_image",
        an_agent_survives_a_reload_and_answers_the_next_image,
    ),
    (
        "the_agent_that_answers_is_the_one_that_was_running_before_the_reload",
        the_agent_that_answers_is_the_one_that_was_running_before_the_reload,
    ),
    (
        "the_handoff_is_not_left_behind_for_a_later_run_to_adopt",
        the_handoff_is_not_left_behind_for_a_later_run_to_adopt,
    ),
    (
        "preparing_a_reload_makes_only_the_master_inheritable",
        preparing_a_reload_makes_only_the_master_inheritable,
    ),
    (
        "nothing_inheritable_is_left_open_after_the_reload_adopts",
        nothing_inheritable_is_left_open_after_the_reload_adopts,
    ),
    (
        "the_next_image_can_take_the_single_instance_lock_again",
        the_next_image_can_take_the_single_instance_lock_again,
    ),
];

fn main() {
    match std::env::var(ROLE_VAR).as_deref() {
        Ok("gen1") => return generation_one(&handoff_from_env()),
        Ok("gen2") => return generation_two(&handoff_from_env()),
        Ok("fd-audit") => return fd_audit(),
        _ => {}
    }

    // Recorded before this runner opens anything of its own, and inherited
    // by every child through the environment.
    // SAFETY: single-threaded at this point; nothing else reads the env yet.
    unsafe { std::env::set_var(AMBIENT_VAR, format_fds(&raw_inheritable_fds())) };

    // The runner. Honours the two things cargo and nextest pass a test binary
    // that matter here: `--list` and a name filter. Every other flag
    // (`--test-threads`, `--nocapture`, ...) is accepted and ignored, since
    // these cases run one after another anyway.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for (name, _) in CASES {
            println!("{name}: test");
        }
        return;
    }
    let exact = args.iter().any(|a| a == "--exact");
    // None of these cases is `#[ignore]`d, so a run of only the ignored ones
    // (`cargo test -- --ignored`) has nothing to do here.
    if args.iter().any(|a| a == "--ignored") {
        println!("\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed");
        return;
    }
    // Positional arguments are name filters, except the value that follows a
    // libtest flag which takes one in the separate-word form.
    const TAKES_VALUE: &[&str] = &[
        "--test-threads",
        "--skip",
        "--format",
        "--color",
        "--logfile",
        "-Z",
    ];
    let mut filters: Vec<&String> = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if TAKES_VALUE.contains(&arg.as_str()) {
            iter.next();
        } else if !arg.starts_with('-') {
            filters.push(arg);
        }
    }
    let selected: Vec<&Case> = CASES
        .iter()
        .filter(|(name, _)| {
            filters.is_empty()
                || filters.iter().any(|f| {
                    if exact {
                        name == f
                    } else {
                        name.contains(f.as_str())
                    }
                })
        })
        .collect();

    println!("\nrunning {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, case) in &selected {
        match std::panic::catch_unwind(case) {
            Ok(()) => println!("test {name} ... ok"),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed.push(*name);
            }
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failed.len(),
        failed.len()
    );
    if !failed.is_empty() {
        std::process::exit(101);
    }
}

fn handoff_from_env() -> PathBuf {
    PathBuf::from(std::env::var(HANDOFF_VAR).expect("handoff path"))
}

// ---------------------------------------------------------------------------
// The two generations of the reload.
// ---------------------------------------------------------------------------

/// Own a live agent, write down how to find it, and hand the process over.
///
/// Drives a real [`PtyClient`], the same type and spawn path dux uses for an
/// agent, so this exercises production's shape rather than a raw descriptor
/// that merely resembles it.
fn generation_one(handoff_path: &Path) {
    // An agent that answers, so the next image can prove it reached this exact
    // process rather than merely holding an open descriptor.
    let client = PtyClient::spawn_with_env(
        "/bin/sh",
        &[
            "-c".to_string(),
            "stty -echo; while IFS= read -r line; do echo \"got:$line\"; done".to_string(),
        ],
        &std::env::current_dir().expect("cwd"),
        24,
        80,
        1000,
        &[],
    )
    .expect("spawn agent");

    // Prepares the descriptor AND describes the pty: exactly what the engine
    // calls for every provider before a reload.
    let entry = client
        .prepare_for_reload("e2e-tab", Some("e2e-session"))
        .expect("prepare the pty for reload");

    println!("GEN1_CHILD_PID={}", entry.child_pid.unwrap_or(0));

    Handoff {
        written_by: std::process::id(),
        ptys: vec![entry],
    }
    .write(handoff_path)
    .expect("write the handoff");

    // Hold the single-instance lock across the exec the way dux does: it lives
    // inside the engine, and the engine is never dropped before the exec.
    if let Ok(lock_path) = std::env::var(LOCK_VAR) {
        let lock = dux_core::lockfile::SingleInstanceLock::acquire(Path::new(&lock_path))
            .expect("generation 1 takes the lock");
        std::mem::forget(lock);
    }

    // The client owns the master. Dropping it would close the descriptor the
    // next image is about to inherit and kill the agent with it, so ownership
    // is deliberately leaked into the exec.
    std::mem::forget(client);

    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().expect("current exe");
    let err = Command::new(exe)
        .env(ROLE_VAR, "gen2")
        .env(HANDOFF_VAR, handoff_path)
        .exec();
    panic!("generation 1 failed to exec: {err}");
}

/// A fresh image that never met the agent, picking it up from the handoff.
fn generation_two(handoff_path: &Path) {
    let handoff = Handoff::consume(handoff_path).expect("read the handoff");
    let entry = handoff.ptys.first().expect("one pty in the handoff");

    // The new image takes the lock the way a fresh dux does. It is the SAME
    // process as the holder, so if the old lock's descriptor had survived the
    // exec this would find its own lock held and refuse (or, with a blocking
    // flock, wait on itself forever).
    if let Ok(lock_path) = std::env::var(LOCK_VAR) {
        let outcome = match dux_core::lockfile::SingleInstanceLock::acquire(Path::new(&lock_path)) {
            Ok(lock) => {
                std::mem::forget(lock);
                "ok".to_string()
            }
            Err(err) => format!("refused: {err}"),
        };
        println!("LOCK_REACQUIRED={outcome}");
    }

    // SAFETY: inherited through the exec; nothing else in this image owns it.
    let client = unsafe {
        PtyClient::adopt_after_reload(
            entry.master_fd,
            entry.child_pid,
            entry.rows,
            entry.cols,
            entry.scrollback_capacity,
            &entry.spawn_dir,
        )
    }
    .expect("rebuild the client from the handoff");

    client.write_bytes(b"ping\n").expect("write to the agent");
    let reply = wait_for_text(&client, "got:ping", std::time::Duration::from_secs(10));

    println!("TAB={}", entry.tab_id);
    println!("SESSION={}", entry.session_id.as_deref().unwrap_or(""));
    // The pid the REBUILT CLIENT holds, not the one copied out of the handoff.
    // Echoing the handoff's own value would agree with itself no matter what
    // the client was actually wired to, so it could not detect a client rebuilt
    // around the wrong process.
    println!("CHILD_PID={}", client.child_process_id().unwrap_or(0));
    println!("CLIENT_LIVE={}", client.is_live());
    println!("REPLY={}", reply.replace('\n', "\\n"));
    // Everything this image holds open that the NEXT exec (or any child it
    // spawns) would inherit. After adoption this must be nothing: the master
    // is ours again and goes back to close-on-exec, and nothing else of the
    // previous image's should have crossed at all.
    println!("GEN2_INHERITABLE={}", format_fds(&inheritable_fds()));
}

/// Every descriptor above stdio that would survive an `exec`.
///
/// Read from `/dev/fd`, the kernel's own list on both macOS and Linux, rather
/// than from what the code under test says it opened, so an fd nobody knew
/// about is caught too. Only meaningful in a single-threaded process, which is
/// why the audits run in their own child of this harness-less binary.
///
/// Descriptors this test binary was itself STARTED with are excluded: whatever
/// launched `cargo test` (a shell, an IDE, an agent harness) may have leaked
/// some into it, and they say nothing about dux. The runner records that set
/// once in [`AMBIENT_VAR`] and every child it spawns subtracts it.
fn inheritable_fds() -> Vec<i32> {
    let ambient: Vec<i32> = std::env::var(AMBIENT_VAR)
        .unwrap_or_default()
        .split(',')
        .filter_map(|n| n.parse().ok())
        .collect();
    let mut fds = raw_inheritable_fds();
    fds.retain(|fd| !ambient.contains(fd));
    fds
}

fn raw_inheritable_fds() -> Vec<i32> {
    let mut fds: Vec<i32> = std::fs::read_dir("/dev/fd")
        .expect("list /dev/fd")
        .filter_map(|e| e.ok()?.file_name().into_string().ok()?.parse().ok())
        .filter(|fd| *fd > 2)
        .filter(|fd| {
            // SAFETY: `F_GETFD` only reads this descriptor's flags. The fd the
            // `read_dir` itself used is closed by now and answers EBADF.
            let flags = unsafe { libc::fcntl(*fd, libc::F_GETFD) };
            flags >= 0 && flags & libc::FD_CLOEXEC == 0
        })
        .collect();
    fds.sort_unstable();
    fds
}

fn format_fds(fds: &[i32]) -> String {
    fds.iter().map(i32::to_string).collect::<Vec<_>>().join(",")
}

/// Spawn an agent, then prepare it for a reload, reporting which descriptors
/// were inheritable before and after.
fn fd_audit() {
    let client = PtyClient::spawn_with_env(
        "/bin/sh",
        &["-c".to_string(), "sleep 30".to_string()],
        &std::env::current_dir().expect("cwd"),
        24,
        80,
        1000,
        &[],
    )
    .expect("spawn agent");
    // A bound listener and an open file alongside, standing in for the
    // background web server's sockets and the lock and log files: none of them
    // may become inheritable, or the new image double-binds and double-locks.
    let _listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a listener");
    let _file = tempfile::tempfile().expect("open a file");
    let before = inheritable_fds();
    let entry = client
        .prepare_for_reload("audit-tab", None)
        .expect("prepare the pty for reload");
    let after = inheritable_fds();
    println!("BEFORE={}", format_fds(&before));
    println!("AFTER={}", format_fds(&after));
    println!("MASTER={}", entry.master_fd);
}

/// Wait until the rebuilt client's terminal shows `needle`.
///
/// Reads the client's own grid rather than the raw descriptor, so a pass proves
/// the reader thread, the terminal emulator and the writer were ALL rebuilt
/// correctly, not merely that the fd is open.
///
/// Bounded: a broken handoff means the agent never answers, and an unbounded
/// wait would hang the test run rather than failing it.
fn wait_for_text(client: &PtyClient, needle: &str, within: std::time::Duration) -> String {
    let deadline = std::time::Instant::now() + within;
    loop {
        let text = client.visible_text_excerpt(200);
        if text.contains(needle) || std::time::Instant::now() >= deadline {
            return text;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

// ---------------------------------------------------------------------------
// The cases.
// ---------------------------------------------------------------------------

/// Run this same binary as generation 1 of a reload and collect what both
/// generations printed.
fn run_reload(handoff: &Path) -> (String, String, bool) {
    let output = Command::new(std::env::current_exe().expect("test exe"))
        .env(ROLE_VAR, "gen1")
        .env(HANDOFF_VAR, handoff)
        .output()
        .expect("run the reload generations");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

fn field<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(key))
        .map(str::trim)
}

fn an_agent_survives_a_reload_and_answers_the_next_image() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_reload(&handoff);
    assert!(ok, "the reload failed:\n{stderr}\n{stdout}");

    // The agent answering is the proof: generation 2 is a different program
    // image that never spawned it and reached it only through the inherited
    // descriptor the handoff named.
    let reply = field(&stdout, "REPLY=").unwrap_or_default();
    assert!(
        reply.contains("got:ping"),
        "the agent started before the reload must answer the image that came \
         after it.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // And it must come back in the row it belonged to.
    assert_eq!(
        field(&stdout, "TAB="),
        Some("e2e-tab"),
        "the rebuilt provider must land in the tab it came from:\n{stdout}"
    );
    assert_eq!(
        field(&stdout, "SESSION="),
        Some("e2e-session"),
        "the session the tab belongs to must survive the handoff:\n{stdout}"
    );

    // The UI decides whether a row is running from the client's own liveness,
    // so a rebuilt client that answered but reported itself dead would show the
    // agent as exited while it is plainly still talking.
    assert_eq!(
        field(&stdout, "CLIENT_LIVE="),
        Some("true"),
        "the rebuilt client must report its adopted agent as live:\n{stdout}"
    );
}

fn the_agent_that_answers_is_the_one_that_was_running_before_the_reload() {
    // Reaching *an* agent is not enough: a reload that silently respawned would
    // also answer. The pid recorded before the exec must be the pid reported
    // after it, which only holds if the original process was carried over.
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_reload(&handoff);
    assert!(ok, "the reload failed:\n{stderr}\n{stdout}");

    let before = field(&stdout, "GEN1_CHILD_PID=").unwrap_or("0");
    let after = field(&stdout, "CHILD_PID=").unwrap_or("0");
    assert_ne!(before, "0", "generation 1 must report the agent it spawned");
    assert_eq!(
        before, after,
        "the agent answering after the reload must be the same process that was \
         running before it, not a fresh one:\n{stdout}"
    );
}

fn the_handoff_is_not_left_behind_for_a_later_run_to_adopt() {
    // The descriptor numbers inside a handoff mean something only to the process
    // that inherited them. A leftover file would have a later run adopt numbers
    // that now belong to something else entirely.
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_reload(&handoff);
    assert!(ok, "the reload failed:\n{stderr}\n{stdout}");

    assert!(
        !handoff.exists(),
        "the image that consumed the handoff must remove it, but {} remains",
        handoff.display()
    );
}

fn preparing_a_reload_makes_only_the_master_inheritable() {
    // Clearing close-on-exec is how a master crosses the exec. It must be the
    // ONLY descriptor that does: anything else left inheritable leaks into the
    // new image (a socket that stays bound, a lock that is never released).
    let output = Command::new(std::env::current_exe().expect("test exe"))
        .env(ROLE_VAR, "fd-audit")
        .output()
        .expect("run the fd audit");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "the fd audit failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let master = field(&stdout, "MASTER=").expect("master fd");
    assert_eq!(
        field(&stdout, "BEFORE="),
        Some(""),
        "nothing may be inheritable before the reload asks for it:\n{stdout}"
    );
    assert_eq!(
        field(&stdout, "AFTER="),
        Some(master),
        "exactly the handed-off master, and nothing else, may lose \
         close-on-exec:\n{stdout}"
    );
}

fn nothing_inheritable_is_left_open_after_the_reload_adopts() {
    // The master crossed the exec with close-on-exec cleared. Once the new
    // image owns it again it must go back on, or every git, gh, editor and
    // agent spawned after a reload inherits a copy of the old agent's terminal
    // (and the reader and writer clones, which are dup'd from it).
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_reload(&handoff);
    assert!(ok, "the reload failed:\n{stderr}\n{stdout}");
    assert_eq!(
        field(&stdout, "GEN2_INHERITABLE="),
        Some(""),
        "after adoption no descriptor may still be inheritable:\n{stdout}"
    );
}

fn the_next_image_can_take_the_single_instance_lock_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());
    let lock = dir.path().join("dux.lock");

    let output = Command::new(std::env::current_exe().expect("test exe"))
        .env(ROLE_VAR, "gen1")
        .env(HANDOFF_VAR, &handoff)
        .env(LOCK_VAR, &lock)
        .output()
        .expect("run the reload generations");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "the reload failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        field(&stdout, "LOCK_REACQUIRED="),
        Some("ok"),
        "the reloaded image must be able to take the lock its predecessor \
         held, or dux refuses to start against itself:\n{stdout}"
    );
}
