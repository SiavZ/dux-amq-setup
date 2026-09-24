//! Helper for the reload end-to-end test: one binary that plays both sides of
//! a reload.
//!
//! The test needs a process that owns a live agent, `exec`s itself, and then
//! talks to that same agent from the replacement image. That cannot be done
//! from inside a `#[test]`: the test binary re-running itself re-enters the
//! test harness, not the helper code, so the exec'd image runs the test suite
//! again instead of picking the agent up. (Observed, not predicted.)
//!
//! Both generations drive a real [`PtyClient`], the same type and the same
//! spawn path dux uses for an agent, so this exercises production's shape
//! rather than a raw descriptor that merely resembles it.

use std::path::{Path, PathBuf};
use std::process::Command;

use dux_core::pty::PtyClient;
use dux_core::reload_handoff::Handoff;

const ROLE_VAR: &str = "DUX_RELOAD_ROLE";
const HANDOFF_VAR: &str = "DUX_RELOAD_HANDOFF";

fn main() {
    let handoff = PathBuf::from(std::env::var(HANDOFF_VAR).expect("handoff path"));
    match std::env::var(ROLE_VAR).as_deref() {
        Ok("gen2") => generation_two(&handoff),
        _ => generation_one(&handoff),
    }
}

/// Own a live agent, write down how to find it, and hand the process over.
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
