//! Helper for the reload end-to-end test: one binary that plays both sides of
//! a reload.
//!
//! The test needs a process that owns a live agent, `exec`s itself, and then
//! talks to that same agent from the replacement image. That cannot be done
//! from inside a `#[test]`: the test binary re-running itself re-enters the
//! test harness, not the helper code, so the exec'd image runs the test suite
//! again instead of picking the agent up.
//!
//! A separate binary has no such ambiguity. `DUX_RELOAD_ROLE` selects the
//! generation, and the whole conversation is reported on stdout for the test to
//! assert on.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use portable_pty::MasterPty;

use dux_core::pty_reattach::{ReattachedMaster, keep_open_across_exec};
use dux_core::reload_handoff::{Handoff, HandoffPty};

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
    let pty = portable_pty::native_pty_system()
        .openpty(portable_pty::PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pty");

    // An agent that answers, so the next image can prove it reached this exact
    // process rather than merely holding an open descriptor.
    let mut cmd = portable_pty::CommandBuilder::new("/bin/sh");
    cmd.args([
        "-c",
        "stty -echo; while IFS= read -r line; do echo \"got:$line\"; done",
    ]);
    let child = pty.slave.spawn_command(cmd).expect("spawn agent");
    drop(pty.slave);

    let fd = pty.master.as_raw_fd().expect("master fd");
    // Without this the kernel closes the master during the exec below and the
    // next image has nothing to adopt.
    keep_open_across_exec(fd).expect("keep the master open across exec");

    Handoff {
        written_by: std::process::id(),
        ptys: vec![HandoffPty {
            tab_id: "e2e-tab".to_string(),
            session_id: Some("e2e-session".to_string()),
            master_fd: fd,
            child_pid: child.process_id(),
            rows: 24,
            cols: 80,
            spawn_dir: std::env::current_dir().expect("cwd"),
            scrollback_capacity: 1000,
        }],
    }
    .write(handoff_path)
    .expect("write the handoff");

    println!("GEN1_CHILD_PID={}", child.process_id().unwrap_or(0));

    // The master owns the descriptor. Dropping it here would close the very fd
    // the next image is about to inherit, so ownership is deliberately leaked
    // into the exec.
    std::mem::forget(pty.master);

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
    let master = unsafe { ReattachedMaster::adopt(entry.master_fd) }.expect("adopt the master");

    let mut writer = master.take_writer().expect("writer");
    writer.write_all(b"ping\n").expect("write to the agent");
    writer.flush().expect("flush");

    let reply = read_until(&master, "got:ping", std::time::Duration::from_secs(10));

    println!("TAB={}", entry.tab_id);
    println!("SESSION={}", entry.session_id.as_deref().unwrap_or(""));
    println!("CHILD_PID={}", entry.child_pid.unwrap_or(0));
    println!("REPLY={}", reply.replace('\n', "\\n"));
}

/// Read until `needle` appears or the deadline passes.
///
/// Non-blocking with a deadline: a broken handoff means the agent never
/// answers, and a blocking read would hang the test run rather than failing it.
fn read_until(master: &ReattachedMaster, needle: &str, within: std::time::Duration) -> String {
    let fd = master.raw_fd();
    // SAFETY: `F_SETFL` only changes this descriptor's status flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let deadline = std::time::Instant::now() + within;
    let mut seen = String::new();
    let mut buf = [0u8; 1024];
    while std::time::Instant::now() < deadline && !seen.contains(needle) {
        // SAFETY: reads at most `buf.len()` bytes into a buffer this scope owns.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n > 0 {
            seen.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    seen
}
