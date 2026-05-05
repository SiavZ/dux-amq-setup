//! Drainer for the AMQ inject-queue.
//!
//! `dux-amq-inject-bridge` (see `dux-amq/scripts/`) writes verified wake
//! bodies to `~/.local/share/dux-amq/inject-queue/<receiver>/<ts>.msg`
//! whenever it detects it's running inside a dux-spawned process tree
//! (via the `DUX_PANE` env exported by `crate::pty::apply_terminal_env`).
//! This module owns:
//!
//! 1. Resolving the queue root from `[amq.inject].queue_dir` (with the
//!    `XDG_DATA_HOME`-aware default).
//! 2. Validating individual queue files (size cap, no symlinks, no
//!    `..` segments).
//! 3. Atomically claiming a file for delivery via a `.inflight.` rename
//!    so concurrent scans don't race.
//! 4. Spawning a `notify` watcher + a polling fallback thread that emit
//!    [`WorkerEvent::AmqInjectScanRequested`] back to the App.
//!
//! The actual "type body into the agent's PTY when idle" logic lives
//! in `crate::app::*` (see `App::tick_amq_inject`). This module is
//! deliberately thin and pure-ish so it's easy to unit-test in
//! isolation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};

use crate::config::AmqInjectConfig;

/// Default queue location when `[amq.inject].queue_dir` is empty.
/// Mirrors the bridge's hard-coded path.
const DEFAULT_QUEUE_REL: &str = ".local/share/dux-amq/inject-queue";

/// The fallback receiver name used by the bridge when `$AM_ME` is unset
/// or sanitises to empty. The drainer treats this directory specially:
/// messages here are delivered to the currently-selected session with
/// a status warning.
pub const UNROUTED_RECEIVER: &str = "_unrouted";

/// Filename prefix marking an in-flight file. The bridge uses
/// `.inflight.XXXXXX` for its own staging temp; the drainer uses
/// `.inflight.<original_basename>` for read-side reservation. Both
/// share the prefix so a single scan filter excludes everything we
/// shouldn't process.
const INFLIGHT_PREFIX: &str = ".inflight.";

/// A queued message ready for delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedMessage {
    /// The receiver name (subdirectory of the queue root). Already
    /// validated to match `[a-z0-9_-]+` or to be the literal
    /// `_unrouted` sentinel.
    pub receiver: String,
    /// Body bytes as written by the bridge, with the bridge's single
    /// trailing newline stripped. Multi-line bodies retain their
    /// interior newlines.
    pub body: String,
    /// Path to the in-flight file (i.e. the file *after* the
    /// `.inflight.` rename). The drainer must `unlink` this on
    /// successful delivery or `release` it on retry.
    pub inflight_path: PathBuf,
    /// Original `<ts>.msg` path before the inflight rename. Carried
    /// for log/diagnostic purposes only.
    pub source_path: PathBuf,
}

/// Reasons a queue file was rejected. Surfaced as
/// [`crate::app::WorkerEvent::AmqInjectError`] so the user sees them
/// in the status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InjectRejection {
    /// Path resolved through a symlink. Blocks symlink-swap attacks
    /// (T11-style) where another process re-points the file between
    /// `stat` and `read`.
    Symlink,
    /// File exceeded `[amq.inject].max_message_bytes`.
    Oversized { actual: u64, cap: u64 },
    /// Receiver dir name failed `[a-z0-9_-]+` check (or was the
    /// reserved `..` token). Means an attacker dropped a file under a
    /// crafted directory name to traverse out of the queue root.
    BadReceiver { name: String },
    /// I/O error while reading the file. The drainer leaves the file
    /// in place so the operator can recover it.
    Io { msg: String },
}

impl InjectRejection {
    pub fn human(&self) -> String {
        match self {
            Self::Symlink => "queue file is a symlink (refusing to follow)".to_string(),
            Self::Oversized { actual, cap } => {
                format!("queue file is {actual} bytes (cap {cap}); skipping")
            }
            Self::BadReceiver { name } => {
                format!("queue subdir name {name:?} is not a valid receiver; skipping")
            }
            Self::Io { msg } => format!("queue file I/O error: {msg}"),
        }
    }
}

/// Resolve the queue root from config. Empty `queue_dir` means
/// `$XDG_DATA_HOME/dux-amq/inject-queue` if `XDG_DATA_HOME` is set, or
/// `~/.local/share/dux-amq/inject-queue` otherwise. The bridge uses the
/// same default — see `dux-amq/scripts/dux-amq-inject-bridge`.
pub fn resolve_queue_dir(config: &AmqInjectConfig) -> Option<PathBuf> {
    if !config.queue_dir.is_empty() {
        return Some(PathBuf::from(&config.queue_dir));
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("dux-amq").join("inject-queue"));
    }
    home::home_dir().map(|h| h.join(DEFAULT_QUEUE_REL))
}

/// True iff `name` is a syntactically valid receiver. The bridge already
/// sanitises to `[a-z0-9_-]+`, so anything else means an attacker (or a
/// stale layout) — refuse to process.
pub fn is_valid_receiver(name: &str) -> bool {
    if name == UNROUTED_RECEIVER {
        return true;
    }
    if name.is_empty() {
        return false;
    }
    if name == "." || name == ".." {
        return false;
    }
    name.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        // Reject leading-dash (defence-in-depth against argv tricks if
        // the receiver ever flows into a CLI argument).
        && !name.starts_with('-')
}

/// Walk the queue root, returning every `<receiver>/<ts>.msg` path
/// that's NOT an `.inflight.*` reservation and NOT a dotfile. Returns
/// rejected entries separately so the caller can emit warnings. Does
/// not recurse beyond the per-receiver dir — multi-level subdirectories
/// inside a receiver are ignored on purpose (forces a flat layout).
pub fn scan_queue_dir(queue_dir: &Path) -> Result<ScanOutcome> {
    let mut messages = Vec::new();
    let mut rejections = Vec::new();
    let entries = match fs::read_dir(queue_dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ScanOutcome::default());
        }
        Err(e) => return Err(anyhow!("read_dir({}): {e}", queue_dir.display())),
    };
    for receiver_entry in entries.flatten() {
        let metadata = match receiver_entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if !metadata.is_dir() {
            continue;
        }
        let name = receiver_entry.file_name();
        let name_str = match name.to_str() {
            Some(s) => s,
            None => continue,
        };
        if !is_valid_receiver(name_str) {
            rejections.push((
                receiver_entry.path(),
                InjectRejection::BadReceiver {
                    name: name_str.to_string(),
                },
            ));
            continue;
        }
        let receiver_dir = receiver_entry.path();
        let inner = match fs::read_dir(&receiver_dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for file_entry in inner.flatten() {
            let file_name = file_entry.file_name();
            let Some(file_str) = file_name.to_str() else {
                continue;
            };
            if file_str.starts_with(INFLIGHT_PREFIX) || file_str.starts_with('.') {
                continue;
            }
            if !file_str.ends_with(".msg") {
                continue;
            }
            messages.push(PendingFile {
                receiver: name_str.to_string(),
                path: file_entry.path(),
            });
        }
    }
    // Stable order: by receiver, then filename. The bridge generates
    // filenames from `+%s%N` so lexical order also matches arrival
    // order. Without the sort, two scans of the same directory could
    // deliver messages in different orders depending on FS behaviour.
    messages.sort_by(|a, b| (&a.receiver, &a.path).cmp(&(&b.receiver, &b.path)));
    Ok(ScanOutcome {
        messages,
        rejections,
    })
}

/// Return value of [`scan_queue_dir`].
#[derive(Default)]
pub struct ScanOutcome {
    pub messages: Vec<PendingFile>,
    pub rejections: Vec<(PathBuf, InjectRejection)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingFile {
    pub receiver: String,
    pub path: PathBuf,
}

/// Atomically claim `path` by renaming it to a sibling with the
/// `.inflight.` prefix. Subsequent scans skip it; on delivery success
/// the caller `unlink`s the in-flight path; on failure
/// [`release`](Self::release) puts it back so the next tick retries.
pub fn claim(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("queue file has no parent: {}", path.display()))?;
    let basename = path
        .file_name()
        .ok_or_else(|| anyhow!("queue file has no basename: {}", path.display()))?
        .to_string_lossy()
        .into_owned();
    let inflight = parent.join(format!("{INFLIGHT_PREFIX}{basename}"));
    fs::rename(path, &inflight)
        .with_context(|| format!("claim {} -> {}", path.display(), inflight.display()))?;
    Ok(inflight)
}

/// Sweep the queue at startup and rename every drainer-format
/// `.inflight.<ts>.msg` file back to `<ts>.msg`. These are leftovers
/// from a prior dux instance that crashed mid-delivery — the
/// single-instance lock guarantees no other dux is currently holding
/// them, so there's no race.
///
/// **Bridge-format temp files (`.inflight.XXXXXX` from `mktemp`,
/// without the `.msg` suffix) are intentionally skipped**: those
/// represent a bridge invocation that's still in the middle of
/// `printf … >$tmp; mv -f $tmp $target`, and renaming them would
/// corrupt the in-flight write. The drainer-format inflight files are
/// always named `.inflight.<original-basename>` where the original
/// basename ends in `.msg`, so the suffix check is the safe
/// differentiator.
///
/// Returns the count of files reclaimed. Errors during a single
/// rename are logged and do not abort the sweep — a partially-stuck
/// queue is still better than no recovery.
pub fn reclaim_stale_inflight(queue_dir: &Path) -> Result<usize> {
    let mut reclaimed = 0usize;
    let entries = match fs::read_dir(queue_dir) {
        Ok(it) => it,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(anyhow!("read_dir({}): {e}", queue_dir.display())),
    };
    for receiver_entry in entries.flatten() {
        if !receiver_entry
            .file_type()
            .map(|t| t.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let receiver_dir = receiver_entry.path();
        let inner = match fs::read_dir(&receiver_dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for entry in inner.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            if !name_str.starts_with(INFLIGHT_PREFIX) {
                continue;
            }
            // Bridge-format mktemp temps don't end in `.msg`; leave
            // them alone so we don't yank a half-written file out
            // from under a concurrent bridge process.
            if !name_str.ends_with(".msg") {
                continue;
            }
            let original_name = &name_str[INFLIGHT_PREFIX.len()..];
            let original_path = receiver_dir.join(original_name);
            let inflight_path = entry.path();
            match fs::rename(&inflight_path, &original_path) {
                Ok(()) => {
                    reclaimed += 1;
                    tracing::info!(
                        target: "dux::amq_inject",
                        path = %original_path.display(),
                        "reclaimed stale inflight from prior dux instance",
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        target: "dux::amq_inject",
                        from = %inflight_path.display(),
                        to = %original_path.display(),
                        err = %e,
                        "stale inflight reclaim failed",
                    );
                }
            }
        }
    }
    Ok(reclaimed)
}

/// Reverse of [`claim`]: rename the in-flight back to its `.msg` form
/// so the next scan picks it up. Used when the PTY write fails (e.g.
/// the agent has exited between the busy-check and the write).
pub fn release(inflight_path: &Path) -> Result<PathBuf> {
    let parent = inflight_path
        .parent()
        .ok_or_else(|| anyhow!("inflight file has no parent: {}", inflight_path.display()))?;
    let basename = inflight_path
        .file_name()
        .ok_or_else(|| anyhow!("inflight file has no basename: {}", inflight_path.display()))?
        .to_string_lossy()
        .into_owned();
    let original = match basename.strip_prefix(INFLIGHT_PREFIX) {
        Some(rest) => parent.join(rest),
        None => return Err(anyhow!("not an inflight file: {basename}")),
    };
    fs::rename(inflight_path, &original).with_context(|| {
        format!(
            "release {} -> {}",
            inflight_path.display(),
            original.display()
        )
    })?;
    Ok(original)
}

/// Read and validate a queue file by path. Caller is expected to have
/// already called [`claim`] so the path here is the `.inflight.` form.
/// Returns the body with one trailing `\n` stripped (mirroring the
/// bridge's `printf '%s\n'`). Rejects symlinks and oversized files.
pub fn read_validated(inflight_path: &Path, max_bytes: u64) -> Result<String, InjectRejection> {
    let metadata = fs::symlink_metadata(inflight_path).map_err(|e| InjectRejection::Io {
        msg: format!("stat {}: {e}", inflight_path.display()),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(InjectRejection::Symlink);
    }
    let size = metadata.len();
    if size > max_bytes {
        return Err(InjectRejection::Oversized {
            actual: size,
            cap: max_bytes,
        });
    }
    let raw = fs::read_to_string(inflight_path).map_err(|e| InjectRejection::Io {
        msg: format!("read {}: {e}", inflight_path.display()),
    })?;
    // The bridge writes `printf '%s\n'`. Strip exactly one trailing
    // LF so multi-line bodies aren't extended.
    let body = raw.strip_suffix('\n').unwrap_or(&raw).to_string();
    Ok(body)
}

/// Spawn the `notify` watcher + polling fallback. The watcher fires
/// `WorkerEvent::AmqInjectScanRequested` whenever any path under
/// `queue_dir` is created or modified; the polling thread fires the
/// same event on a `poll_interval_ms` cadence as a safety net for
/// FSes where `notify` is lossy.
///
/// The watcher is recursive (depth = receiver dir + file). Returns the
/// watcher handle so the caller can keep it alive — dropping it stops
/// the inotify thread.
pub fn spawn_inject_watcher<E>(
    queue_dir: PathBuf,
    poll_interval_ms: u64,
    event_tx: Sender<E>,
    make_event: impl Fn() -> E + Send + 'static + Copy,
) -> Result<Arc<Mutex<RecommendedWatcher>>>
where
    E: Send + 'static,
{
    fs::create_dir_all(&queue_dir)
        .with_context(|| format!("creating amq-inject queue dir {}", queue_dir.display()))?;
    let notify_tx = event_tx.clone();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            let Ok(event) = res else { return };
            // We care about file creates and writes only. Removes are
            // handled by the drainer itself when it unlinks delivered
            // files; we don't need to re-scan in response.
            if !event.kind.is_create() && !event.kind.is_modify() {
                return;
            }
            let _ = notify_tx.send(make_event());
        },
        NotifyConfig::default(),
    )
    .context("creating amq-inject notify watcher")?;
    watcher
        .watch(&queue_dir, RecursiveMode::Recursive)
        .with_context(|| format!("watching {}", queue_dir.display()))?;
    let watcher = Arc::new(Mutex::new(watcher));

    // Polling fallback: send a scan request every `poll_interval_ms`.
    // We don't bother debouncing against the notify path because the
    // App-side scan is cheap (`read_dir` + filter) and dedups via the
    // pending_inject map.
    let poll_tx = event_tx;
    let interval = Duration::from_millis(poll_interval_ms.max(100));
    thread::Builder::new()
        .name("dux-amq-inject-poll".to_string())
        .spawn(move || {
            loop {
                thread::sleep(interval);
                if poll_tx.send(make_event()).is_err() {
                    // Receiver gone — App is shutting down.
                    break;
                }
            }
        })
        .context("spawning amq-inject poll thread")?;

    // Kick the App once at startup so any messages queued while dux
    // wasn't running get drained on next tick.
    Ok(watcher)
}

/// Heuristic: is the agent currently busy (streaming, awaiting tool
/// approval, etc.) given a recent PTY snapshot? Returns `true` if any
/// configured `busy_marker` substring appears in the snapshot. Plain
/// substring match — no regex — because the markers are operator-
/// configurable and we want literal matching to be predictable.
pub fn snapshot_indicates_busy(snapshot: &str, busy_markers: &[String]) -> bool {
    busy_markers.iter().any(|marker| snapshot.contains(marker))
}

/// Truncate a body for status-line display. Operates on chars (not
/// bytes) to avoid splitting a multibyte boundary. Per the project
/// tenet: "Never use byte-based `.len()` or `[..n]` slicing to truncate
/// user-visible strings."
pub fn preview(body: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars + 1);
    for (idx, ch) in body.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        if ch == '\n' || ch == '\r' {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn is_valid_receiver_accepts_sanitised_handles() {
        assert!(is_valid_receiver("alice"));
        assert!(is_valid_receiver("watch-rules-phase3"));
        assert!(is_valid_receiver("a1b2_c3"));
        assert!(is_valid_receiver(UNROUTED_RECEIVER));
    }

    #[test]
    fn is_valid_receiver_rejects_traversal_and_dotfiles() {
        assert!(!is_valid_receiver(""));
        assert!(!is_valid_receiver("."));
        assert!(!is_valid_receiver(".."));
        assert!(!is_valid_receiver("../etc"));
        assert!(!is_valid_receiver("etc/passwd"));
        assert!(!is_valid_receiver(".hidden"));
        assert!(!is_valid_receiver("UPPER"));
        assert!(!is_valid_receiver("with space"));
        assert!(!is_valid_receiver("-leading-dash"));
    }

    #[test]
    fn scan_queue_dir_returns_empty_when_root_missing() {
        let dir = tempdir().unwrap();
        let queue = dir.path().join("inject-queue");
        let outcome = scan_queue_dir(&queue).unwrap();
        assert!(outcome.messages.is_empty());
        assert!(outcome.rejections.is_empty());
    }

    #[test]
    fn scan_queue_dir_finds_msg_files_and_skips_inflight() {
        let dir = tempdir().unwrap();
        let queue = dir.path().to_path_buf();
        let bob = queue.join("bob");
        fs::create_dir_all(&bob).unwrap();
        fs::write(bob.join("001.msg"), b"first\n").unwrap();
        fs::write(bob.join("002.msg"), b"second\n").unwrap();
        // In-flight files (either side's reservation) must be skipped.
        fs::write(bob.join(".inflight.001.msg"), b"in-flight\n").unwrap();
        fs::write(bob.join(".inflight.abc"), b"bridge-temp\n").unwrap();
        // Hidden files (e.g. an editor swap) ignored.
        fs::write(bob.join(".hidden.msg"), b"hidden\n").unwrap();
        // Non-`.msg` extensions ignored.
        fs::write(bob.join("README.txt"), b"readme\n").unwrap();

        let outcome = scan_queue_dir(&queue).unwrap();
        assert_eq!(outcome.messages.len(), 2);
        assert_eq!(outcome.messages[0].receiver, "bob");
        assert!(
            outcome.messages[0]
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("001.msg")
        );
    }

    #[test]
    fn scan_queue_dir_rejects_bad_receiver_dirs() {
        let dir = tempdir().unwrap();
        let queue = dir.path().to_path_buf();
        // An attacker-crafted directory name with mixed case + dots.
        let bad = queue.join("Eve.evil");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("001.msg"), b"steal\n").unwrap();
        let outcome = scan_queue_dir(&queue).unwrap();
        assert!(outcome.messages.is_empty());
        assert_eq!(outcome.rejections.len(), 1);
        assert!(matches!(
            outcome.rejections[0].1,
            InjectRejection::BadReceiver { .. }
        ));
    }

    #[test]
    fn read_validated_strips_single_trailing_newline() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("msg.msg");
        fs::write(&path, b"hello world\n").unwrap();
        assert_eq!(
            read_validated(&path, 1024).unwrap(),
            "hello world".to_string()
        );
    }

    #[test]
    fn read_validated_preserves_interior_newlines() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("msg.msg");
        fs::write(&path, b"line one\nline two\nline three\n").unwrap();
        assert_eq!(
            read_validated(&path, 1024).unwrap(),
            "line one\nline two\nline three".to_string()
        );
    }

    #[test]
    fn read_validated_rejects_oversized_files() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("big.msg");
        fs::write(&path, vec![b'x'; 1024]).unwrap();
        let result = read_validated(&path, 100);
        match result {
            Err(InjectRejection::Oversized { actual, cap }) => {
                assert_eq!(actual, 1024);
                assert_eq!(cap, 100);
            }
            other => panic!("expected Oversized, got {other:?}"),
        }
    }

    #[test]
    #[cfg(unix)]
    fn read_validated_rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = tempdir().unwrap();
        let target = dir.path().join("real.msg");
        fs::write(&target, b"body\n").unwrap();
        let link = dir.path().join("link.msg");
        symlink(&target, &link).unwrap();
        let result = read_validated(&link, 1024);
        assert!(matches!(result, Err(InjectRejection::Symlink)));
    }

    #[test]
    fn reclaim_stale_inflight_renames_drainer_format_files() {
        let dir = tempdir().unwrap();
        let queue = dir.path().to_path_buf();
        let alice = queue.join("alice");
        fs::create_dir_all(&alice).unwrap();

        // Drainer-format inflight (orphaned by a prior crash).
        let stale_drainer = alice.join(".inflight.001.msg");
        fs::write(&stale_drainer, b"orphan-body\n").unwrap();

        // Bridge-format mktemp temp (concurrent in-flight write —
        // MUST be left alone or we corrupt the bridge's printf+mv).
        let bridge_temp = alice.join(".inflight.aBcXyZ");
        fs::write(&bridge_temp, b"in-progress\n").unwrap();

        // Regular live message — must not be touched.
        let live = alice.join("002.msg");
        fs::write(&live, b"new-arrival\n").unwrap();

        let n = reclaim_stale_inflight(&queue).unwrap();
        assert_eq!(n, 1, "expected exactly one drainer-format reclaim");

        let recovered = alice.join("001.msg");
        assert!(
            recovered.exists(),
            "drainer-format inflight should be renamed back"
        );
        assert!(!stale_drainer.exists());
        assert!(
            bridge_temp.exists(),
            "bridge-format mktemp temp must NOT be renamed"
        );
        assert!(live.exists(), "live messages must not be touched");
    }

    #[test]
    fn reclaim_stale_inflight_handles_missing_queue_dir() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let n = reclaim_stale_inflight(&missing).unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn reclaim_stale_inflight_skips_non_dir_entries_in_queue_root() {
        let dir = tempdir().unwrap();
        let queue = dir.path().to_path_buf();
        // A stray file at the root level (could be a leftover from
        // the legacy flat layout or operator detritus). Reclaim must
        // not fail on it or try to enter it as a dir.
        fs::write(queue.join("README.txt"), b"hello").unwrap();
        let alice = queue.join("alice");
        fs::create_dir_all(&alice).unwrap();
        fs::write(alice.join(".inflight.001.msg"), b"x").unwrap();
        let n = reclaim_stale_inflight(&queue).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn reclaim_stale_inflight_continues_past_per_file_errors() {
        let dir = tempdir().unwrap();
        let queue = dir.path().to_path_buf();
        let alice = queue.join("alice");
        fs::create_dir_all(&alice).unwrap();
        // Two reclaimable files; first one's destination already
        // exists (collision — should be logged + skipped).
        fs::write(alice.join(".inflight.001.msg"), b"a").unwrap();
        fs::write(alice.join("001.msg"), b"existing").unwrap(); // collision target
        fs::write(alice.join(".inflight.002.msg"), b"b").unwrap();
        let n = reclaim_stale_inflight(&queue).unwrap();
        // POSIX `rename` overwrites the destination, so collision
        // produces a successful rename. Both reclaim. Document this
        // explicitly so a future change to e.g. `renameat2(NOREPLACE)`
        // surfaces here.
        assert_eq!(n, 2);
    }

    #[test]
    fn claim_and_release_round_trip() {
        let dir = tempdir().unwrap();
        let original = dir.path().join("alice").join("001.msg");
        fs::create_dir_all(original.parent().unwrap()).unwrap();
        fs::write(&original, b"hi\n").unwrap();

        let inflight = claim(&original).unwrap();
        assert!(inflight.exists());
        assert!(!original.exists());
        assert!(
            inflight
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(INFLIGHT_PREFIX)
        );

        let restored = release(&inflight).unwrap();
        assert_eq!(restored, original);
        assert!(original.exists());
        assert!(!inflight.exists());
    }

    #[test]
    fn snapshot_indicates_busy_matches_substring() {
        let markers = vec!["esc to interrupt".to_string()];
        let busy = "Working… (24s, esc to interrupt)";
        let idle = "│ > Type your message here";
        assert!(snapshot_indicates_busy(busy, &markers));
        assert!(!snapshot_indicates_busy(idle, &markers));
    }

    #[test]
    fn snapshot_indicates_busy_empty_markers_means_never_busy() {
        let markers: Vec<String> = vec![];
        let busy = "Working… (24s, esc to interrupt)";
        assert!(!snapshot_indicates_busy(busy, &markers));
    }

    #[test]
    fn preview_truncates_to_char_boundary() {
        let body = "héllo wörld with 中文 chars";
        let result = preview(body, 5);
        assert_eq!(result.chars().count(), 6); // 5 chars + ellipsis
    }

    #[test]
    fn preview_replaces_newlines_with_spaces() {
        let body = "line one\nline two";
        let result = preview(body, 100);
        assert_eq!(result, "line one line two");
    }

    #[test]
    fn resolve_queue_dir_uses_xdg_data_home_when_set() {
        // Save and restore env so the test doesn't clobber the
        // ambient XDG_DATA_HOME for parallel tests.
        let saved = std::env::var("XDG_DATA_HOME").ok();
        // SAFETY: rust 2024 marks env mutation as unsafe; we only
        // touch our own var and restore it before exit. Tests in
        // this module are not parallelised against XDG.
        unsafe {
            std::env::set_var("XDG_DATA_HOME", "/custom/xdg");
        }
        let cfg = AmqInjectConfig::default();
        let resolved = resolve_queue_dir(&cfg).unwrap();
        assert_eq!(resolved, PathBuf::from("/custom/xdg/dux-amq/inject-queue"));
        unsafe {
            match saved {
                Some(v) => std::env::set_var("XDG_DATA_HOME", v),
                None => std::env::remove_var("XDG_DATA_HOME"),
            }
        }
    }

    #[test]
    fn resolve_queue_dir_honours_explicit_override() {
        let cfg = AmqInjectConfig {
            queue_dir: "/var/spool/dux-amq".to_string(),
            ..AmqInjectConfig::default()
        };
        assert_eq!(
            resolve_queue_dir(&cfg).unwrap(),
            PathBuf::from("/var/spool/dux-amq")
        );
    }
}
