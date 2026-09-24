//! What one dux image hands the next one when it reloads in place.
//!
//! # The problem this solves
//!
//! A reload replaces the running binary with `exec`. The agents survive, because
//! `exec` keeps the process's children, and their PTY masters can survive with
//! them (see [`crate::pty_reattach`]). What does not survive is memory: the
//! replacement image starts with no idea which descriptors it inherited, which
//! agent each one belongs to, or how big its terminal was.
//!
//! So the outgoing image writes that down, and the incoming one reads it. This
//! module is that note and nothing more. It deliberately holds only what cannot
//! be recovered any other way:
//!
//! - the descriptor number, which is meaningless once the old image is gone
//!   unless someone records it
//! - which tab and session it belongs to, so the rebuilt provider lands in the
//!   right row
//! - the child's pid and the terminal geometry, so the new image can describe
//!   the agent without re-deriving it
//!
//! Session rows, projects and worktrees are NOT here: those already live in
//! sqlite and survive a restart on their own. Copying them into the handoff
//! would create a second source of truth that could disagree with the first.
//!
//! # Why a file rather than an environment variable
//!
//! The manifest grows with the number of live agents, and an environment block
//! has a size limit that a busy session could realistically reach. A file has no
//! such ceiling, can be inspected after a failed reload, and is removed by the
//! image that consumes it.

use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Names the handoff file on the incoming image's command line.
///
/// An explicit flag rather than a well-known path: two dux instances can be
/// mid-reload at once, and a fixed filename would have them read each other's
/// notes.
pub const HANDOFF_FLAG: &str = "--reload-handoff";

/// One live agent, described well enough to be rebuilt after the exec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffPty {
    /// The tab this PTY belongs to, as `TabId::as_str`.
    pub tab_id: String,
    /// The session the tab belongs to, for rejoining the row to its session.
    pub session_id: Option<String>,
    /// The inherited master descriptor.
    ///
    /// Only meaningful inside the process that inherited it, which is why the
    /// manifest is written immediately before the exec and consumed immediately
    /// after: a number from any other process, or from a previous run, names
    /// something else entirely.
    pub master_fd: RawFd,
    /// The agent process's pid, carried so the new image can report on the child
    /// it adopted without having to rediscover it.
    pub child_pid: Option<u32>,
    pub rows: u16,
    pub cols: u16,
    /// The directory the child was spawned in.
    pub spawn_dir: PathBuf,
    /// The scrollback ring capacity the PTY was created with, in lines. Fixed at
    /// construction, so the rebuilt terminal has to be given the same value
    /// rather than today's config value.
    pub scrollback_capacity: usize,
}

/// Everything the outgoing image knows that the incoming one cannot re-derive.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Handoff {
    /// The pid that wrote this, so a manifest left behind by a different process
    /// is recognisable rather than silently adopted.
    pub written_by: u32,
    pub ptys: Vec<HandoffPty>,
}

impl Handoff {
    /// Write the manifest to `path`.
    ///
    /// Written whole through a temporary file and renamed into place, so a
    /// reload that dies mid-write leaves either the old file or none, never a
    /// half-parsed one that the next image would act on.
    pub fn write(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_vec_pretty(self).context("encoding the reload handoff")?;
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, &json)
            .with_context(|| format!("writing the reload handoff to {}", temp.display()))?;
        std::fs::rename(&temp, path)
            .with_context(|| format!("moving the reload handoff into {}", path.display()))?;
        Ok(())
    }

    /// Read the manifest at `path`, and remove it.
    ///
    /// Removal is part of reading on purpose: the descriptor numbers inside are
    /// only meaningful to the one process that inherited them, so a manifest
    /// that outlived its exec is worse than no manifest at all. Consuming it
    /// here means a later run cannot find it and adopt numbers that now belong
    /// to something else.
    pub fn consume(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("reading the reload handoff at {}", path.display()))?;
        // Removed before parsing, so a corrupt manifest is not left to be
        // retried by the next run either.
        let _ = std::fs::remove_file(path);
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing the reload handoff at {}", path.display()))
    }

    /// Where to put a handoff for the process that is about to exec.
    ///
    /// Keyed by pid so two dux instances reloading at the same moment cannot
    /// overwrite each other's manifest.
    pub fn path_for(dir: &Path, pid: u32) -> PathBuf {
        dir.join(format!("reload-handoff-{pid}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Handoff {
        Handoff {
            written_by: 4242,
            ptys: vec![HandoffPty {
                tab_id: "s1-slot".to_string(),
                session_id: Some("s1".to_string()),
                master_fd: 7,
                child_pid: Some(991),
                rows: 24,
                cols: 80,
                spawn_dir: PathBuf::from("/tmp/work"),
                scrollback_capacity: 1000,
            }],
        }
    }

    #[test]
    fn a_handoff_survives_the_round_trip_it_exists_for() {
        // The manifest crosses an exec as bytes on disk, so the only property
        // that matters is that what comes back is what went in.
        let dir = tempfile::tempdir().unwrap();
        let path = Handoff::path_for(dir.path(), 4242);
        let original = sample();
        original.write(&path).unwrap();

        let restored = Handoff::consume(&path).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn reading_a_handoff_consumes_it() {
        // The fd numbers inside mean something only to the process that
        // inherited them. A manifest left on disk would be read by a later run
        // whose fd 7 is some unrelated file, so reading must remove it.
        let dir = tempfile::tempdir().unwrap();
        let path = Handoff::path_for(dir.path(), 4242);
        sample().write(&path).unwrap();

        Handoff::consume(&path).unwrap();
        assert!(
            !path.exists(),
            "a consumed handoff must not be left for a later run to adopt"
        );
    }

    #[test]
    fn a_corrupt_handoff_is_refused_and_not_left_behind() {
        // A truncated write (a reload that died mid-flight) must not be retried
        // forever by every subsequent run.
        let dir = tempfile::tempdir().unwrap();
        let path = Handoff::path_for(dir.path(), 4242);
        std::fs::write(&path, b"{ this is not json").unwrap();

        assert!(Handoff::consume(&path).is_err());
        assert!(
            !path.exists(),
            "a corrupt handoff must be cleared, not retried"
        );
    }

    #[test]
    fn two_instances_reloading_at_once_do_not_share_a_file() {
        // Keyed by pid precisely so a second dux mid-reload cannot overwrite the
        // first one's descriptor numbers.
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(
            Handoff::path_for(dir.path(), 1),
            Handoff::path_for(dir.path(), 2)
        );
    }

    #[test]
    fn a_partial_write_never_becomes_the_visible_file() {
        // `write` renames into place, so a reader either sees the previous
        // manifest or none: never a half-written one it would act on.
        let dir = tempfile::tempdir().unwrap();
        let path = Handoff::path_for(dir.path(), 4242);
        sample().write(&path).unwrap();
        // The temporary name must not survive a successful write.
        assert!(
            !path.with_extension("tmp").exists(),
            "the staging file must be renamed away, not left beside the real one"
        );
    }
}
