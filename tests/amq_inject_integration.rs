//! End-to-end integration test for the AMQ inject-queue drainer.
//!
//! Spawns `cat` in a real PTY (via `dux::pty::PtyClient`), writes a
//! queue file to a tempdir, and drives the drainer's public API
//! (`scan_queue_dir`, `claim`, `read_validated`) to deliver the body
//! through the PTY and verify the bytes round-trip. Mirrors the
//! pattern in `tests/watch_engine_integration.rs`.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use dux::amq_inject::{
    InjectRejection, claim, read_validated, reclaim_stale_inflight, scan_queue_dir,
};
use dux::pty::PtyClient;

fn wait_until<F: FnMut() -> bool>(mut cond: F, timeout: Duration, step: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(step);
    }
    cond()
}

fn write_msg(queue_root: &std::path::Path, receiver: &str, name: &str, body: &str) -> PathBuf {
    let dir = queue_root.join(receiver);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    // Match the bridge's `printf '%s\n'` semantics — single trailing LF.
    fs::write(&path, format!("{body}\n")).unwrap();
    path
}

#[test]
fn drainer_delivers_body_to_pty_and_unlinks_file() {
    // Spawn `cat` so writes echo back. Same pattern as the watch
    // engine integration test.
    let cwd = std::env::temp_dir();
    let client = PtyClient::spawn("cat", &[], &cwd, 24, 80, 1_000).expect("spawn cat in PTY");

    // Build a queue dir under a tempdir + drop a single `.msg` file.
    let tmp = tempfile::tempdir().expect("tempdir");
    let queue_root = tmp.path().to_path_buf();
    let original = write_msg(&queue_root, "alice", "001.msg", "please continue working");

    // Scan picks up exactly one queued message under "alice/".
    let outcome = scan_queue_dir(&queue_root).expect("scan");
    assert_eq!(outcome.messages.len(), 1);
    assert!(
        outcome.rejections.is_empty(),
        "no rejections expected, got {:?}",
        outcome.rejections
    );
    let pending = &outcome.messages[0];
    assert_eq!(pending.receiver, "alice");
    assert_eq!(pending.path, original);

    // Claim renames to `.inflight.<name>`; original is gone.
    let inflight = claim(&pending.path).expect("claim");
    assert!(inflight.exists());
    assert!(!original.exists());

    // Read+validate strips the trailing LF.
    let body = read_validated(&inflight, 65_536).expect("validated read");
    assert_eq!(body, "please continue working");

    // Deliver: body + CR. `cat` echoes everything we write so we can
    // see it land in the PTY snapshot.
    let mut payload = body.into_bytes();
    payload.push(b'\r');
    client.write_bytes(&payload).expect("PTY write");

    let saw_echo = wait_until(
        || {
            client
                .scan_recent_lines(30)
                .contains("please continue working")
        },
        Duration::from_secs(2),
        Duration::from_millis(20),
    );
    assert!(
        saw_echo,
        "cat should echo the delivered body within 2s; actual: {:?}",
        client.scan_recent_lines(30)
    );

    // After successful delivery the drainer unlinks the inflight file.
    fs::remove_file(&inflight).expect("unlink inflight");
    let outcome2 = scan_queue_dir(&queue_root).expect("scan after delivery");
    assert!(
        outcome2.messages.is_empty(),
        "queue should be empty after delivery, found {:?}",
        outcome2.messages
    );
}

#[test]
fn drainer_rejects_oversized_messages_at_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let queue_root = tmp.path().to_path_buf();
    // 200-byte body — over the 100-byte cap we'll pass to validate.
    let big_body: String = "x".repeat(200);
    let path = write_msg(&queue_root, "alice", "001.msg", &big_body);
    let outcome = scan_queue_dir(&queue_root).unwrap();
    let pending = outcome.messages.first().expect("one message");
    let inflight = claim(&pending.path).expect("claim");
    let result = read_validated(&inflight, 100);
    match result {
        Err(InjectRejection::Oversized { actual, cap }) => {
            assert_eq!(cap, 100);
            // `printf '%s\n'` adds one extra byte for the LF.
            assert!(actual >= 200, "actual size {actual} should include body+LF");
        }
        other => panic!("expected Oversized, got {other:?}"),
    }
    // Defence-in-depth: original `.msg` was claimed, so it's no longer
    // visible to subsequent scans (the `.inflight.` file remains for
    // the operator to inspect).
    assert!(!path.exists());
    assert!(inflight.exists());
}

#[test]
fn drainer_skips_inflight_files_during_scan() {
    let tmp = tempfile::tempdir().unwrap();
    let queue_root = tmp.path().to_path_buf();
    let alice = queue_root.join("alice");
    fs::create_dir_all(&alice).unwrap();
    fs::write(alice.join("001.msg"), "real body\n").unwrap();
    fs::write(alice.join(".inflight.002.msg"), "drainer-claimed\n").unwrap();
    fs::write(alice.join(".inflight.XXXXX"), "bridge-temp\n").unwrap();

    let outcome = scan_queue_dir(&queue_root).unwrap();
    assert_eq!(outcome.messages.len(), 1);
    assert!(
        outcome.messages[0]
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("001.msg")
    );
}

/// End-to-end recovery: simulate a prior dux instance that crashed
/// mid-claim by leaving a `.inflight.<ts>.msg` file behind. Run the
/// reclaim sweep, scan, claim, validate, write through a PTY, and
/// confirm the body actually lands in the agent's input. This is the
/// exact path that orphaned a real message in production before this
/// fix.
#[test]
fn reclaim_then_deliver_recovers_orphan() {
    let cwd = std::env::temp_dir();
    let client = PtyClient::spawn("cat", &[], &cwd, 24, 80, 1_000).expect("spawn cat in PTY");

    let tmp = tempfile::tempdir().expect("tempdir");
    let queue_root = tmp.path().to_path_buf();
    let alice = queue_root.join("alice");
    fs::create_dir_all(&alice).unwrap();

    // Pretend a prior dux claimed this message and then died.
    let stale_inflight = alice.join(".inflight.001.msg");
    fs::write(&stale_inflight, "recovered orphan body\n").unwrap();

    // Reclaim sweep — this is what App::spawn_amq_inject_watcher
    // calls before the watcher starts.
    let n = reclaim_stale_inflight(&queue_root).expect("reclaim");
    assert_eq!(n, 1);
    let restored = alice.join("001.msg");
    assert!(restored.exists());
    assert!(!stale_inflight.exists());

    // Now the regular scan + claim + deliver path picks it up.
    let outcome = scan_queue_dir(&queue_root).unwrap();
    assert_eq!(outcome.messages.len(), 1);
    let inflight = claim(&outcome.messages[0].path).expect("claim");
    let body = read_validated(&inflight, 65_536).expect("validated read");
    assert_eq!(body, "recovered orphan body");

    let mut payload = body.into_bytes();
    payload.push(b'\r');
    client.write_bytes(&payload).expect("PTY write");
    let saw_echo = wait_until(
        || {
            client
                .scan_recent_lines(30)
                .contains("recovered orphan body")
        },
        Duration::from_secs(2),
        Duration::from_millis(20),
    );
    assert!(
        saw_echo,
        "cat should echo the reclaimed body within 2s; actual: {:?}",
        client.scan_recent_lines(30)
    );
}
