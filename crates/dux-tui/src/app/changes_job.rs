//! Changes-panel git mutations (stage, unstage, discard, commit) run off the
//! run loop.
//!
//! Each of these shells out to git, and git can stall on an index lock, a slow
//! filesystem or a commit hook, which used to freeze the whole interface for
//! the duration (fork b91ed679 / 773a6b04 P1-23, and the CLAUDE.md rule that
//! every potentially blocking git command runs on a worker). The git work
//! happens on a thread; the run loop only folds the answer back in:
//! the status line, the changed-files refresh, and the UI follow-ups that used
//! to run inline after the call.
//!
//! One slot: the changes pane acts on one worktree at a time, and a second
//! mutation fired while the first is still running would race it on the index.
//! A request while one is in flight is refused with a warning instead.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use dux_core::statusline::StatusTone;

use super::{App, RightSection};

/// The status key the in-flight job's spinner rides on.
pub(crate) const CHANGES_JOB_STATUS_KEY: &str = "changes-job";

/// What the worker is asked to do.
#[derive(Clone, Debug)]
pub(crate) enum ChangesJob {
    Stage {
        path: String,
    },
    Unstage {
        path: String,
    },
    /// Classify tracked-vs-untracked against LIVE status, then discard. Both
    /// halves run together on the worker so the decision and the destructive
    /// action see the same worktree.
    Discard {
        path: String,
    },
    /// Preflight (reads live status) and then commit.
    Commit {
        message: String,
        success_message: String,
    },
}

impl ChangesJob {
    fn busy_message(&self) -> String {
        match self {
            Self::Stage { path } => format!("Staging \"{path}\"\u{2026}"),
            Self::Unstage { path } => format!("Unstaging \"{path}\"\u{2026}"),
            Self::Discard { path } => format!("Discarding changes to \"{path}\"\u{2026}"),
            Self::Commit { .. } => "Committing staged changes\u{2026}".to_string(),
        }
    }
}

/// What came back.
#[derive(Debug)]
pub(crate) struct ChangesJobAnswer {
    pub(crate) job: ChangesJob,
    /// `Ok(Some(msg))` shows an info line, `Ok(None)` shows nothing (a stage or
    /// unstage is its own feedback), `Err` shows an error line.
    pub(crate) result: Result<Option<String>, String>,
}

pub(crate) struct PendingChangesJob {
    pub(crate) rx: mpsc::Receiver<ChangesJobAnswer>,
}

/// The git half. Runs on the worker thread; pure apart from git, so the unit
/// tests below drive it directly.
pub(crate) fn run_changes_job(
    worktree: &std::path::Path,
    job: &ChangesJob,
) -> Result<Option<String>, String> {
    use dux_core::git;
    match job {
        ChangesJob::Stage { path } => git::stage_file(worktree, path)
            .map(|()| None)
            .map_err(|e| format!("Stage failed: {e:#}")),
        ChangesJob::Unstage { path } => git::unstage_file(worktree, path)
            .map(|()| None)
            .map_err(|e| format!("Unstage failed: {e:#}")),
        ChangesJob::Discard { path } => {
            let is_untracked = git::discard_classify(worktree, path)
                .map_err(|e| format!("Discard failed: {e:#}"))?;
            git::discard_file(worktree, path, is_untracked)
                .map_err(|e| format!("Discard failed: {e:#}"))?;
            Ok(Some(if is_untracked {
                format!("Deleted untracked file \"{path}\".")
            } else {
                format!(
                    "Discarded unstaged changes to \"{path}\". Staged changes, if any, are kept."
                )
            }))
        }
        ChangesJob::Commit {
            message,
            success_message,
        } => {
            match git::commit_preflight(worktree, message) {
                git::CommitPreflight::EmptyMessage => {
                    return Err("Enter a commit message first.".to_string());
                }
                git::CommitPreflight::NothingStaged => {
                    return Err("No staged changes to commit.".to_string());
                }
                git::CommitPreflight::Ready => {}
            }
            git::commit(worktree, message)
                .map(|_| Some(success_message.clone()))
                .map_err(|e| format!("Commit failed: {e}"))
        }
    }
}

/// Where the files cursor goes after a stage or unstage, decided from the
/// lists as git reports them AFTER the operation: the section it was in has
/// emptied and the other one has something in it. `None` stays put. Discard
/// and commit never move it (fork 773a6b04's `section_after_git_file_op`,
/// expressed on post-reload counts).
pub(crate) fn section_after_stage_toggle(
    current: RightSection,
    staged: usize,
    unstaged: usize,
) -> Option<RightSection> {
    match current {
        RightSection::Unstaged if unstaged == 0 && staged > 0 => Some(RightSection::Staged),
        RightSection::Staged if staged == 0 && unstaged > 0 => Some(RightSection::Unstaged),
        _ => None,
    }
}

impl App {
    /// Start a changes-panel mutation on a worker. Returns `false` (and says so
    /// on the status line) when another one is still running.
    pub(crate) fn dispatch_changes_job(&mut self, worktree: PathBuf, job: ChangesJob) -> bool {
        if self.pending_changes_job.is_some() {
            self.set_warning("A git change is still running; try again in a moment.");
            return false;
        }
        self.status.set(
            Instant::now(),
            Some(CHANGES_JOB_STATUS_KEY.to_string()),
            StatusTone::Busy,
            job.busy_message(),
        );
        let (tx, rx) = mpsc::channel();
        let job_for_worker = job.clone();
        let wt = worktree;
        let spawned = std::thread::Builder::new()
            .name("dux-changes-job".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_changes_job(&wt, &job_for_worker)
                }))
                .unwrap_or_else(|_| Err("The git worker panicked.".to_string()));
                let _ = tx.send(ChangesJobAnswer {
                    job: job_for_worker,
                    result,
                });
            });
        match spawned {
            Ok(_) => {
                self.pending_changes_job = Some(PendingChangesJob { rx });
                true
            }
            Err(err) => {
                self.status.clear(CHANGES_JOB_STATUS_KEY, None);
                self.set_error(format!("Could not start background worker: {err}"));
                false
            }
        }
    }

    /// Fold a finished changes job back in. Called once per run-loop tick.
    pub(crate) fn drain_changes_job(&mut self) {
        let Some(pending) = self.pending_changes_job.as_ref() else {
            return;
        };
        let answer = match pending.rx.try_recv() {
            Ok(answer) => answer,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_changes_job = None;
                self.status.clear(CHANGES_JOB_STATUS_KEY, None);
                self.set_error("The git worker exited without an answer.");
                return;
            }
        };
        self.pending_changes_job = None;
        self.apply_changes_job_answer(answer);
    }

    fn apply_changes_job_answer(&mut self, answer: ChangesJobAnswer) {
        self.mark_frame_dirty();
        self.status.clear(CHANGES_JOB_STATUS_KEY, None);
        let ok = answer.result.is_ok();
        match answer.result {
            Ok(Some(message)) => self.set_info(message),
            Ok(None) => {}
            Err(message) => self.set_error(message),
        }
        match answer.job {
            ChangesJob::Stage { .. } | ChangesJob::Unstage { .. } => {
                self.reload_changed_files();
                if let Some(section) = section_after_stage_toggle(
                    self.right_section,
                    self.engine.staged_files.len(),
                    self.engine.unstaged_files.len(),
                ) {
                    self.right_section = section;
                    self.clamp_files_cursor();
                }
            }
            ChangesJob::Discard { .. } => {
                if ok {
                    self.reload_changed_files();
                }
            }
            ChangesJob::Commit { .. } => {
                if ok {
                    self.commit_input.clear();
                    self.reload_changed_files();
                }
            }
        }
    }

    /// Test helper: block until the in-flight job answers, then fold it in.
    #[cfg(test)]
    pub(crate) fn finish_changes_job(&mut self) {
        if let Some(pending) = self.pending_changes_job.take() {
            let answer = pending
                .rx
                .recv_timeout(std::time::Duration::from_secs(20))
                .expect("changes job answered");
            self.apply_changes_job_answer(answer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let git = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .expect("git");
            assert!(out.status.success(), "git {args:?}: {out:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "a.txt"]);
        git(&["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn stage_commit_and_discard_run_through_the_job() {
        let dir = repo();
        let wt = dir.path();
        std::fs::write(wt.join("a.txt"), "two\n").unwrap();
        assert_eq!(
            run_changes_job(
                wt,
                &ChangesJob::Stage {
                    path: "a.txt".into()
                }
            ),
            Ok(None)
        );
        let msg = run_changes_job(
            wt,
            &ChangesJob::Commit {
                message: "second".into(),
                success_message: "Committed.".into(),
            },
        );
        assert_eq!(msg, Ok(Some("Committed.".to_string())));

        std::fs::write(wt.join("new.txt"), "x\n").unwrap();
        let msg = run_changes_job(
            wt,
            &ChangesJob::Discard {
                path: "new.txt".into(),
            },
        )
        .unwrap();
        assert!(msg.unwrap().contains("Deleted untracked"));
        assert!(!wt.join("new.txt").exists());
    }

    #[test]
    fn commit_refusals_come_from_the_live_preflight() {
        let dir = repo();
        let job = |message: &str| ChangesJob::Commit {
            message: message.into(),
            success_message: "ok".into(),
        };
        assert_eq!(
            run_changes_job(dir.path(), &job("  ")),
            Err("Enter a commit message first.".to_string())
        );
        assert_eq!(
            run_changes_job(dir.path(), &job("msg")),
            Err("No staged changes to commit.".to_string())
        );
    }

    // Fork 773a6b04 section-advance tests, on post-reload counts.

    #[test]
    fn staging_last_unstaged_file_advances_to_staged() {
        // Before: 2 staged, 1 unstaged. After staging it: 3 staged, 0 unstaged.
        assert_eq!(
            section_after_stage_toggle(RightSection::Unstaged, 3, 0),
            Some(RightSection::Staged)
        );
    }

    #[test]
    fn unstaging_last_staged_file_advances_to_unstaged() {
        assert_eq!(
            section_after_stage_toggle(RightSection::Staged, 0, 4),
            Some(RightSection::Unstaged)
        );
    }

    #[test]
    fn no_advance_when_source_section_keeps_entries() {
        assert_eq!(
            section_after_stage_toggle(RightSection::Unstaged, 2, 1),
            None
        );
        assert_eq!(section_after_stage_toggle(RightSection::Staged, 2, 2), None);
    }

    #[test]
    fn no_advance_when_destination_section_empty() {
        // Both lists empty (a stage that git answered with nothing to show):
        // there is no better section to move to.
        assert_eq!(
            section_after_stage_toggle(RightSection::Unstaged, 0, 0),
            None
        );
        assert_eq!(section_after_stage_toggle(RightSection::Staged, 0, 0), None);
    }

    #[test]
    fn discard_never_advances() {
        // Discard does not consult the rule at all; the commit box never moves.
        assert_eq!(
            section_after_stage_toggle(RightSection::CommitInput, 0, 0),
            None
        );
    }
}
