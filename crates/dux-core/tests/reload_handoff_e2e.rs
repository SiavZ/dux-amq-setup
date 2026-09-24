//! The reload handoff, end to end, across a real `exec`.
//!
//! The unit tests for `dux_core::pty_reattach` and `dux_core::reload_handoff`
//! each check their own half: that a descriptor can be adopted, and that a
//! manifest survives a round trip through a file. Neither can check the thing
//! the feature actually promises, because that only happens when a process
//! really replaces itself and a DIFFERENT program image picks the agent back up.
//!
//! That is what this does, through the `reload_handoff_helper` binary: it starts
//! an agent on a PTY, writes a handoff, and `exec`s itself. The second image
//! reads the handoff, adopts the inherited descriptor, and talks to the agent the
//! first image started. If any link is wrong (the descriptor closed by the exec,
//! the manifest lost, the adopted fd pointing elsewhere) the conversation fails
//! and so does this test.

use std::path::{Path, PathBuf};
use std::process::Command;

use dux_core::reload_handoff::Handoff;

/// Path to the helper binary, which cargo builds next to the test binary.
///
/// Derived from this test's own executable rather than hardcoded, so it is
/// correct under `debug`, `release`, a custom `--target-dir`, and cross builds
/// alike.
fn helper_binary() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test exe");
    dir.pop(); // the test binary's own file name
    if dir.ends_with("deps") {
        dir.pop();
    }
    let candidate = dir.join("reload_handoff_helper");
    assert!(
        candidate.exists(),
        "the reload helper binary is missing at {}; it is a bin target of \
         dux-core and should be built alongside this test",
        candidate.display()
    );
    candidate
}

fn run_helper(handoff: &Path) -> (String, String, bool) {
    let output = Command::new(helper_binary())
        .env("DUX_RELOAD_HANDOFF", handoff)
        .output()
        .expect("run the reload helper");
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

#[test]
fn an_agent_survives_a_reload_and_answers_the_next_image() {
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_helper(&handoff);
    assert!(ok, "the reload helper failed:\n{stderr}\n{stdout}");

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
}

#[test]
fn the_agent_that_answers_is_the_one_that_was_running_before_the_reload() {
    // Reaching *an* agent is not enough: a reload that silently respawned would
    // also answer. The pid recorded before the exec must be the pid reported
    // after it, which only holds if the original process was carried over.
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_helper(&handoff);
    assert!(ok, "the reload helper failed:\n{stderr}\n{stdout}");

    let before = field(&stdout, "GEN1_CHILD_PID=").unwrap_or("0");
    let after = field(&stdout, "CHILD_PID=").unwrap_or("0");
    assert_ne!(before, "0", "generation 1 must report the agent it spawned");
    assert_eq!(
        before, after,
        "the agent answering after the reload must be the same process that was \
         running before it, not a fresh one:\n{stdout}"
    );
}

#[test]
fn the_handoff_is_not_left_behind_for_a_later_run_to_adopt() {
    // The descriptor numbers inside a handoff mean something only to the process
    // that inherited them. A leftover file would have a later run adopt numbers
    // that now belong to something else entirely.
    let dir = tempfile::tempdir().expect("tempdir");
    let handoff = Handoff::path_for(dir.path(), std::process::id());

    let (stdout, stderr, ok) = run_helper(&handoff);
    assert!(ok, "the reload helper failed:\n{stderr}\n{stdout}");

    assert!(
        !handoff.exists(),
        "the image that consumed the handoff must remove it, but {} remains",
        handoff.display()
    );
}
