//! Whether a reload may proceed, and onto which binary.
//!
//! The transport (`dux_core::reload_handoff` and friends) can hand live agents
//! across an `exec`. This module answers the question that comes first: should
//! it, right now?
//!
//! Both rules are borrowed from jcode, which has run this in production:
//! it refuses while a turn is in flight (`if self.is_processing { return false }`)
//! and only reloads when the binary on disk is actually newer
//! (`has_newer_binary`). The reasons are the same here.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Why a reload was refused, in the words the user sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadRefusal {
    /// An agent is mid-turn.
    AgentWorking { tab_id: String },
    /// The binary on disk is the one already running.
    NoNewerBinary,
    /// A PTY could not be prepared for the handoff, so the exec would strand it.
    HandoffUnavailable,
}

impl ReloadRefusal {
    /// The sentence to show. Each one says what stopped the reload and what the
    /// user can do, because "reload failed" alone leaves them guessing.
    pub fn message(&self) -> String {
        match self {
            Self::AgentWorking { tab_id } => format!(
                "Not reloading: {tab_id} is still working. Reloading now would \
                 cut off its output mid-turn. Try again when it settles."
            ),
            Self::NoNewerBinary => {
                "Already running the newest dux on disk, so there is nothing to \
                 reload onto."
                    .to_string()
            }
            Self::HandoffUnavailable => {
                "Not reloading: one of the running agents could not be handed \
                 over, and reloading would leave it running with nothing \
                 attached to it."
                    .to_string()
            }
        }
    }
}

/// Whether `candidate` is a different build from the one running.
///
/// Compares modification times rather than versions, because the case this
/// serves is a developer who just rebuilt: the version string is usually
/// unchanged, and the mtime is what actually moved.
///
/// `running_started` is the mtime of the binary THIS process was launched from,
/// captured at startup. Reading it live would be wrong: a rebuild replaces that
/// file in place, so by the time the user asks, the path points at the new
/// binary and the comparison would say "not newer" about the very build they
/// want.
pub fn binary_is_newer(candidate: &Path, running_started: Option<SystemTime>) -> bool {
    let Some(running) = running_started else {
        // With no baseline, nothing can be shown to be newer. Refusing is the
        // safe direction: a reload the user did not need costs them a cleared
        // transcript for nothing.
        return false;
    };
    std::fs::metadata(candidate)
        .and_then(|m| m.modified())
        .is_ok_and(|candidate_mtime| candidate_mtime > running)
}

/// The binary a reload would exec onto, and its mtime when the process started.
///
/// Captured once at startup precisely because the file can be replaced
/// underneath a running process.
#[derive(Debug, Clone)]
pub struct ReloadTarget {
    pub path: PathBuf,
    pub started_mtime: Option<SystemTime>,
}

impl ReloadTarget {
    /// Read the current executable and stamp its mtime now.
    pub fn capture() -> Option<Self> {
        let path = std::env::current_exe().ok()?;
        let started_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Some(Self {
            path,
            started_mtime,
        })
    }

    /// Whether the file at [`path`](Self::path) has been replaced since startup.
    pub fn has_newer_build(&self) -> bool {
        binary_is_newer(&self.path, self.started_mtime)
    }
}

/// Replace this process with `exe`, telling it to adopt the handoff at
/// `handoff_path`.
///
/// On success this NEVER RETURNS: `exec` replaces the running image in place,
/// which is what keeps the process (and therefore its children and their
/// descriptors) alive across the swap. An `Err` means the exec itself failed and
/// this image is still running, with its agents untouched.
///
/// The caller must have written the handoff and prepared every descriptor
/// before calling, because after this there is no opportunity to.
pub fn exec_into_reload(
    exe: &Path,
    handoff_path: &Path,
) -> std::io::Result<std::convert::Infallible> {
    use std::os::unix::process::CommandExt;

    let err = std::process::Command::new(exe)
        .arg(crate::reload_handoff::HANDOFF_FLAG)
        .arg(handoff_path)
        .exec();
    // `exec` only returns on failure.
    Err(err)
}

/// Read `--reload-handoff <path>` out of a command line.
///
/// Returns `None` when the flag is absent, which is every normal launch. A flag
/// with no path after it is also `None`: a malformed invocation must start
/// normally rather than fail, because there is no handoff to lose.
pub fn handoff_path_from_args<I, S>(args: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg.as_ref() == crate::reload_handoff::HANDOFF_FLAG {
            return args.next().map(|p| PathBuf::from(p.as_ref()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_file(path: &Path, body: &[u8]) {
        let mut f = std::fs::File::create(path).expect("create");
        f.write_all(body).expect("write");
        f.sync_all().expect("sync");
    }

    #[test]
    fn a_rebuilt_binary_reads_as_newer() {
        // The case this exists for: a developer rebuilds in place, so the path
        // is unchanged and only the mtime moved.
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("dux");
        write_file(&exe, b"old");
        let started = std::fs::metadata(&exe)
            .and_then(|m| m.modified())
            .expect("mtime");

        assert!(
            !binary_is_newer(&exe, Some(started)),
            "an untouched binary must not read as newer"
        );

        // A filesystem may only record whole seconds, so move the timestamp
        // explicitly rather than racing it with a sleep.
        let later = started + std::time::Duration::from_secs(2);
        let f = std::fs::File::options()
            .write(true)
            .open(&exe)
            .expect("reopen");
        f.set_modified(later).expect("set mtime");
        drop(f);

        assert!(
            binary_is_newer(&exe, Some(started)),
            "a binary replaced since startup must read as newer"
        );
    }

    #[test]
    fn an_older_binary_does_not_count_as_newer() {
        // Rolling back to a previous build leaves an OLDER mtime. Treating any
        // difference as "newer" would reload in a loop between two builds.
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("dux");
        write_file(&exe, b"x");
        let started = std::fs::metadata(&exe)
            .and_then(|m| m.modified())
            .expect("mtime");

        let earlier = started - std::time::Duration::from_secs(60);
        let f = std::fs::File::options()
            .write(true)
            .open(&exe)
            .expect("reopen");
        f.set_modified(earlier).expect("set mtime");
        drop(f);

        assert!(
            !binary_is_newer(&exe, Some(started)),
            "an older binary is not a reload target"
        );
    }

    #[test]
    fn a_missing_binary_is_not_a_reload_target() {
        // The path can vanish (an install that unlinks and rewrites). Nothing to
        // exec onto means nothing to offer.
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!binary_is_newer(
            &dir.path().join("gone"),
            Some(SystemTime::now())
        ));
    }

    #[test]
    fn without_a_baseline_nothing_reads_as_newer() {
        // No baseline means no evidence. Refusing costs the user nothing; a
        // spurious reload costs them their transcript.
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("dux");
        write_file(&exe, b"x");
        assert!(!binary_is_newer(&exe, None));
    }

    #[test]
    fn every_refusal_says_what_to_do_about_it() {
        // A refusal the user cannot act on is a dead end, so each one names both
        // the cause and the way forward.
        let working = ReloadRefusal::AgentWorking {
            tab_id: "s1-slot".to_string(),
        };
        assert!(working.message().contains("s1-slot"));
        assert!(
            working.message().contains("Try again"),
            "a busy refusal must tell the user it is worth retrying"
        );

        assert!(
            ReloadRefusal::NoNewerBinary.message().contains("newest"),
            "the no-op case must say the binary is already current"
        );
        assert!(
            ReloadRefusal::HandoffUnavailable
                .message()
                .contains("could not be handed over"),
            "the handoff failure must name what went wrong"
        );
    }

    #[test]
    fn the_handoff_flag_is_read_off_the_command_line() {
        // The incoming image learns it is a reload from this flag alone.
        assert_eq!(
            handoff_path_from_args(["--reload-handoff", "/tmp/h.json"]),
            Some(PathBuf::from("/tmp/h.json"))
        );
        // Position must not matter: other arguments may precede it.
        assert_eq!(
            handoff_path_from_args(["--quiet", "--reload-handoff", "/tmp/h.json"]),
            Some(PathBuf::from("/tmp/h.json"))
        );
    }

    #[test]
    fn a_normal_launch_carries_no_handoff() {
        // Every ordinary start goes through here, so absence must be cheap and
        // unambiguous rather than an error.
        assert_eq!(handoff_path_from_args(["--quiet"]), None);
        assert_eq!(handoff_path_from_args(Vec::<String>::new()), None);
    }

    #[test]
    fn a_flag_with_no_path_starts_normally_instead_of_failing() {
        // A truncated command line has no handoff to lose, so the honest
        // response is an ordinary cold start, not a refusal to run at all.
        assert_eq!(handoff_path_from_args(["--reload-handoff"]), None);
    }
}
