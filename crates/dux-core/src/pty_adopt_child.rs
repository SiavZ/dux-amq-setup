//! The child half of re-attaching after a reload.
//!
//! [`crate::pty_reattach`] adopts the PTY master; this adopts the process on the
//! other end of it. `PtyClient` holds its child as a `Box<dyn Child + Send +
//! Sync>`, and the value the original spawn produced lived in the previous
//! image's memory, so the replacement image needs one built from the pid alone.
//!
//! # This really is still our child
//!
//! Worth stating plainly, because the opposite is the natural assumption: `exec`
//! replaces the program image but NOT the process, so every parent-child
//! relationship it had survives. The agent is not reparented to init and does
//! not become unwaitable. Verified directly: after an exec, the replacement
//! image sent `SIGKILL` to the agent started by its predecessor and `waitpid`
//! reaped it normally.
//!
//! So this is a thin wrapper, not an emulation. `kill` and `waitpid` are the
//! same calls the original `Child` made; only the Rust value describing the
//! relationship had to be rebuilt.

use std::io::{Error, Result};

use portable_pty::{Child, ChildKiller, ExitStatus};

/// A child process adopted from a previous image after a reload.
#[derive(Debug)]
pub struct AdoptedChild {
    pid: libc::pid_t,
    /// The status from the first successful reap, so later polls can answer too.
    ///
    /// `waitpid` yields a status exactly once: the second call finds no zombie
    /// and reports `ECHILD`. `PtyClient` polls from several places, so without
    /// this the first caller would consume the status out from under the rest
    /// and every later one would report "unknown".
    reaped: Option<ExitStatus>,
}

impl AdoptedChild {
    pub fn new(pid: u32) -> Self {
        Self {
            pid: pid as libc::pid_t,
            reaped: None,
        }
    }

    /// One `waitpid`, optionally blocking. `Ok(None)` means still running.
    fn reap(&mut self, block: bool) -> Result<Option<ExitStatus>> {
        if let Some(status) = self.reaped.as_ref() {
            return Ok(Some(status.clone()));
        }
        let mut raw: libc::c_int = 0;
        let flags = if block { 0 } else { libc::WNOHANG };
        // SAFETY: `waitpid` writes only through the `raw` pointer this call owns.
        let rc = unsafe { libc::waitpid(self.pid, &mut raw, flags) };
        if rc == 0 {
            return Ok(None); // WNOHANG: alive.
        }
        if rc < 0 {
            let err = Error::last_os_error();
            // ECHILD means nothing is left to wait for: the process is gone and
            // someone else reaped it. Reporting an error would make an exited
            // agent look like a broken one, so it is recorded as an exit whose
            // code could not be observed.
            if err.raw_os_error() == Some(libc::ECHILD) {
                let status = ExitStatus::with_exit_code(0);
                self.reaped = Some(status.clone());
                return Ok(Some(status));
            }
            return Err(err);
        }
        let status = exit_status_from_raw(raw);
        self.reaped = Some(status.clone());
        Ok(Some(status))
    }
}

/// Translate a raw `waitpid` status into an exit code.
///
/// A signalled process is reported as `128 + signal`, the shell convention, so a
/// killed agent is distinguishable from one that exited with a small code rather
/// than being flattened into the same number.
fn exit_status_from_raw(raw: libc::c_int) -> ExitStatus {
    // Read with the libc macros rather than by hand: the bit layout is
    // platform-specific and differs between Linux and the BSDs.
    if libc::WIFEXITED(raw) {
        ExitStatus::with_exit_code(libc::WEXITSTATUS(raw) as u32)
    } else if libc::WIFSIGNALED(raw) {
        ExitStatus::with_exit_code(128 + libc::WTERMSIG(raw) as u32)
    } else {
        ExitStatus::with_exit_code(0)
    }
}

impl Child for AdoptedChild {
    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        self.reap(false)
    }

    fn wait(&mut self) -> Result<ExitStatus> {
        self.reap(true)?
            .ok_or_else(|| Error::other("blocking wait returned without a status"))
    }

    fn process_id(&self) -> Option<u32> {
        // A reaped pid is no longer a live process and may be reused, so it is
        // not reported as one.
        self.reaped.is_none().then_some(self.pid as u32)
    }
}

impl ChildKiller for AdoptedChild {
    fn kill(&mut self) -> Result<()> {
        // The process GROUP, matching what a normally spawned child gets:
        // portable_pty calls `setsid`, so the agent leads its own group and a
        // signal aimed at the bare pid would leave its descendants running.
        // SAFETY: `killpg` only signals; a gone group fails with ESRCH.
        let rc = unsafe { libc::killpg(self.pid, libc::SIGKILL) };
        if rc != 0 {
            let err = Error::last_os_error();
            // ESRCH means it is already gone, which is the outcome kill wanted.
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err);
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(AdoptedChildKiller { pid: self.pid })
    }
}

/// A killer split out from an [`AdoptedChild`], so a thread blocked in `wait`
/// can still be signalled from elsewhere.
#[derive(Debug)]
struct AdoptedChildKiller {
    pid: libc::pid_t,
}

impl ChildKiller for AdoptedChildKiller {
    fn kill(&mut self) -> Result<()> {
        // SAFETY: `killpg` only signals.
        let rc = unsafe { libc::killpg(self.pid, libc::SIGKILL) };
        if rc != 0 {
            let err = Error::last_os_error();
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err);
            }
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(AdoptedChildKiller { pid: self.pid })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real process this test owns, so the wrapper is exercised against the
    /// same kind of child a reload adopts rather than a fabricated pid.
    fn spawn_sleeper(seconds: &str) -> std::process::Child {
        std::process::Command::new("sleep")
            .arg(seconds)
            .spawn()
            .expect("spawn sleep")
    }

    #[test]
    fn a_running_child_reports_as_running() {
        let mut real = spawn_sleeper("30");
        let mut adopted = AdoptedChild::new(real.id());

        assert!(
            adopted.try_wait().expect("try_wait").is_none(),
            "a live child must report as still running"
        );
        assert_eq!(adopted.process_id(), Some(real.id()));

        let _ = real.kill();
        let _ = real.wait();
    }

    #[test]
    fn an_exited_child_yields_its_code_to_every_caller() {
        // `waitpid` hands back a status exactly once. PtyClient polls from
        // several places, so the wrapper has to remember it or the first poll
        // steals the exit code from all the others.
        let mut real = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .spawn()
            .expect("spawn");
        let mut adopted = AdoptedChild::new(real.id());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut first = None;
        while std::time::Instant::now() < deadline {
            if let Some(status) = adopted.try_wait().expect("try_wait") {
                first = Some(status);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let first = first.expect("the child must be reaped within the deadline");
        assert_eq!(first.exit_code(), 7, "the real exit code must be reported");

        let second = adopted
            .try_wait()
            .expect("try_wait")
            .expect("a second caller must still see the status");
        assert_eq!(
            second.exit_code(),
            7,
            "the status must be remembered, not consumed by the first caller"
        );

        // `std`'s own handle is stale now; reaping again is expected to fail.
        let _ = real.try_wait();
    }

    #[test]
    fn a_reaped_child_stops_reporting_a_process_id() {
        // A reaped pid can be reused by the kernel for an unrelated process, so
        // continuing to report it would name somebody else's.
        let mut real = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let mut adopted = AdoptedChild::new(real.id());

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline
            && adopted.try_wait().expect("try_wait").is_none()
        {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            adopted.process_id().is_none(),
            "a reaped child must not keep reporting its pid"
        );
        let _ = real.try_wait();
    }

    #[test]
    fn killing_an_already_dead_child_is_not_an_error() {
        // Teardown runs on paths where the agent may already have exited. A
        // refusal there would turn an ordinary race into a reported failure.
        let mut real = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn");
        let pid = real.id();
        let _ = real.wait();

        let mut adopted = AdoptedChild::new(pid);
        assert!(
            adopted.kill().is_ok(),
            "killing a child that is already gone must succeed quietly"
        );
    }

    #[test]
    fn a_signalled_child_is_distinguishable_from_a_low_exit_code() {
        // 128 + signal, the shell convention. Without it a SIGKILLed agent and
        // an agent that exited 9 would be indistinguishable in the UI.
        let mut real = spawn_sleeper("30");
        let mut adopted = AdoptedChild::new(real.id());
        let _ = real.kill();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut status = None;
        while std::time::Instant::now() < deadline {
            if let Some(s) = adopted.try_wait().expect("try_wait") {
                status = Some(s);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let status = status.expect("the killed child must be reaped");
        assert_eq!(
            status.exit_code(),
            128 + libc::SIGKILL as u32,
            "a signalled child must report 128 + signal, not a bare code"
        );
        let _ = real.try_wait();
    }
}
