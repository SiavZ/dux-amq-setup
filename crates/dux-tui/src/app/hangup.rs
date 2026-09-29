//! Leaving when the terminal is gone.
//!
//! A TUI's terminal can disappear underneath it: the window is closed, the SSH
//! connection drops, the terminal emulator is killed. The kernel hangs the tty
//! up and sends SIGHUP, and the user expects what they expect of any terminal
//! program, which is that it ends.
//!
//! Two things here make that true:
//!
//! - [`wait_for_terminal_input`] is how the run loop waits for a key. It
//!   reports a hung-up terminal as [`TerminalInput::Gone`], so the loop quits
//!   instead of handing a dead tty to crossterm. Crossterm's reader has no way
//!   out of one: a read of zero bytes, or any error but `WouldBlock`, sends its
//!   loop round again, forever, and the run loop that polls the shutdown flag
//!   never gets the thread back.
//! - [`QuitWatchdog`] is the backstop. It watches the same shutdown flag from
//!   its own thread, and if a quit was asked for and the run loop has not left
//!   within the grace, it ends the process. Whatever the main thread is stuck
//!   in, a signal that asks dux to quit is never just swallowed.

use std::os::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};

/// What waiting on the terminal found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalInput {
    /// Nothing arrived within the timeout.
    Idle,
    /// There are bytes to read.
    Ready,
    /// The terminal hung up, or its descriptor is no longer a usable one.
    /// There will never be input again.
    Gone,
}

/// Read a poll result. A hang-up wins over readable: a hung-up tty is also
/// "readable", and what it has to read is the end of the file.
pub(crate) fn classify_terminal_poll(revents: PollFlags) -> TerminalInput {
    if revents.intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL) {
        TerminalInput::Gone
    } else if revents.contains(PollFlags::IN) {
        TerminalInput::Ready
    } else {
        TerminalInput::Idle
    }
}

/// Wait up to `timeout` for input on the terminal behind `fd`.
///
/// An interrupted wait is retried, as everywhere else in the run loop: a
/// signal handler only sets a flag, and the loop reads that flag at the top of
/// its next turn.
pub(crate) fn wait_for_terminal_input(
    fd: impl AsFd,
    timeout: Duration,
) -> rustix::io::Result<TerminalInput> {
    let timeout = rustix::time::Timespec {
        tv_sec: timeout.as_secs().try_into().unwrap_or(i64::MAX),
        tv_nsec: i64::from(timeout.subsec_nanos()),
    };
    let fd = fd.as_fd();
    crate::io_retry::retry_on_interrupt_errno(|| {
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        let ready = poll(&mut fds, Some(&timeout))?;
        Ok(if ready == 0 {
            TerminalInput::Idle
        } else {
            classify_terminal_poll(fds[0].revents())
        })
    })
}

/// How long the run loop has to act on a quit before the watchdog does.
///
/// The loop turns at least ten times a second, so a healthy one leaves within
/// a fraction of this. It is long enough that a loop busy with one slow frame
/// is never mistaken for a stuck one.
pub(crate) const QUIT_WATCHDOG_GRACE: Duration = Duration::from_secs(5);

const QUIT_WATCHDOG_TICK: Duration = Duration::from_millis(200);

/// What the watchdog makes of one look at the flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WatchdogVerdict {
    /// No quit has been asked for, or the grace is still running.
    Wait,
    /// The run loop left: there is nothing to watch any more.
    Retire,
    /// A quit was asked for and the run loop did not act on it in time.
    Wedged,
}

/// The watchdog's rule, as a pure function of what it can see. `asked_at` is
/// when it first saw the quit request still unanswered.
pub(crate) fn watchdog_verdict(
    run_loop_left: bool,
    asked_at: Option<Instant>,
    now: Instant,
    grace: Duration,
) -> WatchdogVerdict {
    if run_loop_left {
        return WatchdogVerdict::Retire;
    }
    match asked_at {
        Some(asked_at) if now.saturating_duration_since(asked_at) >= grace => {
            WatchdogVerdict::Wedged
        }
        _ => WatchdogVerdict::Wait,
    }
}

/// Watches for a quit the run loop never acted on.
///
/// Holds nothing of the app's: only the two flags. That is what lets it work
/// when the main thread is stuck, and it is also why the most it can do is end
/// the process. Ending it closes every agent's PTY, so each agent is hung up
/// exactly as it is when its terminal window closes.
pub(crate) struct QuitWatchdog {
    run_loop_left: Arc<AtomicBool>,
}

impl QuitWatchdog {
    /// Start watching `quit_requested`. `on_wedged` runs on the watchdog's
    /// thread when the run loop fails to leave within `grace`; outside tests it
    /// is [`exit_because_wedged`].
    pub(crate) fn spawn(
        quit_requested: Arc<AtomicBool>,
        grace: Duration,
        on_wedged: impl FnOnce() + Send + 'static,
    ) -> Self {
        let run_loop_left = Arc::new(AtomicBool::new(false));
        let left = Arc::clone(&run_loop_left);
        let spawned = thread::Builder::new()
            .name("dux-quit-watchdog".to_string())
            .spawn(move || {
                let mut asked_at = None;
                loop {
                    if asked_at.is_none() && quit_requested.load(Ordering::Relaxed) {
                        asked_at = Some(Instant::now());
                    }
                    match watchdog_verdict(
                        left.load(Ordering::Relaxed),
                        asked_at,
                        Instant::now(),
                        grace,
                    ) {
                        WatchdogVerdict::Wait => thread::sleep(QUIT_WATCHDOG_TICK.min(grace)),
                        WatchdogVerdict::Retire => return,
                        WatchdogVerdict::Wedged => {
                            on_wedged();
                            return;
                        }
                    }
                }
            });
        if let Err(err) = spawned {
            // Without the thread dux is where it was before the watchdog
            // existed, which is no reason to refuse to start.
            crate::logger::warn(&format!("could not start the quit watchdog: {err}"));
        }
        Self { run_loop_left }
    }

    /// The run loop returned, for whatever reason. From here the shutdown
    /// path owns the process, and its own grace period applies.
    pub(crate) fn run_loop_left(&self) {
        self.run_loop_left.store(true, Ordering::Relaxed);
    }
}

impl Drop for QuitWatchdog {
    fn drop(&mut self) {
        self.run_loop_left();
    }
}

/// End the process because the run loop did not act on a quit.
///
/// The status is 128 + SIGHUP, what a shell reports for a program the hang-up
/// ended: a quit the loop cannot act on is, in practice, a vanished terminal.
pub(crate) fn exit_because_wedged() {
    crate::logger::error(&format!(
        "a quit was requested but the interface did not respond within {}s; ending dux so \
         it cannot linger without a terminal. Agents are hung up with it and resume on the \
         next start.",
        QUIT_WATCHDOG_GRACE.as_secs()
    ));
    std::process::exit(129);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::mpsc;

    /// A pseudo terminal, as the two ends of it.
    fn open_pty() -> (OwnedFd, OwnedFd) {
        let mut master = 0;
        let mut slave = 0;
        // SAFETY: both out-pointers are valid for the call, and the three
        // optional arguments are allowed to be null.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0, "openpty failed");
        // SAFETY: openpty returned two fresh descriptors this test owns.
        unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) }
    }

    #[test]
    fn a_hang_up_wins_over_readable() {
        assert_eq!(
            classify_terminal_poll(PollFlags::IN | PollFlags::HUP),
            TerminalInput::Gone
        );
        assert_eq!(classify_terminal_poll(PollFlags::HUP), TerminalInput::Gone);
        assert_eq!(classify_terminal_poll(PollFlags::ERR), TerminalInput::Gone);
        assert_eq!(classify_terminal_poll(PollFlags::NVAL), TerminalInput::Gone);
        assert_eq!(classify_terminal_poll(PollFlags::IN), TerminalInput::Ready);
        assert_eq!(
            classify_terminal_poll(PollFlags::empty()),
            TerminalInput::Idle
        );
    }

    /// The three things a real terminal can be: quiet, typed into, and closed.
    /// The closed one is the window being shut with dux still in it.
    #[test]
    fn a_closed_terminal_is_reported_gone() {
        let (master, slave) = open_pty();
        let short = Duration::from_millis(20);

        assert_eq!(
            wait_for_terminal_input(&slave, short).unwrap(),
            TerminalInput::Idle
        );

        rustix::io::write(&master, b"x\n").unwrap();
        assert_eq!(
            wait_for_terminal_input(&slave, Duration::from_secs(2)).unwrap(),
            TerminalInput::Ready
        );

        drop(master);
        assert_eq!(
            wait_for_terminal_input(&slave, Duration::from_secs(2)).unwrap(),
            TerminalInput::Gone
        );
    }

    #[test]
    fn the_watchdog_waits_out_the_grace_and_no_longer() {
        let grace = Duration::from_secs(5);
        let asked = Instant::now();
        let at = |secs: u64| asked + Duration::from_secs(secs);

        // Nothing asked for: nothing to do, however long it has been.
        assert_eq!(
            watchdog_verdict(false, None, at(60), grace),
            WatchdogVerdict::Wait
        );
        // Asked, and the loop still has time.
        assert_eq!(
            watchdog_verdict(false, Some(asked), at(4), grace),
            WatchdogVerdict::Wait
        );
        // Asked, and the time is up.
        assert_eq!(
            watchdog_verdict(false, Some(asked), at(5), grace),
            WatchdogVerdict::Wedged
        );
        // The loop left: whatever else is true, the watchdog is done.
        assert_eq!(
            watchdog_verdict(true, Some(asked), at(60), grace),
            WatchdogVerdict::Retire
        );
    }

    /// The case this exists for: the quit flag is set and the run loop never
    /// comes back to read it.
    #[test]
    fn the_watchdog_acts_when_a_quit_is_never_answered() {
        let quit = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let _watchdog =
            QuitWatchdog::spawn(Arc::clone(&quit), Duration::from_millis(60), move || {
                let _ = tx.send(());
            });

        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "no quit was asked for, so there is nothing to act on"
        );
        quit.store(true, Ordering::Relaxed);
        rx.recv_timeout(Duration::from_secs(5))
            .expect("an unanswered quit must end in the watchdog acting");
    }

    /// A healthy quit: the loop reads the flag and leaves, and the watchdog
    /// stays out of the shutdown that follows, however long that takes.
    #[test]
    fn the_watchdog_stays_out_of_a_quit_the_run_loop_answered() {
        let quit = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let watchdog =
            QuitWatchdog::spawn(Arc::clone(&quit), Duration::from_millis(60), move || {
                let _ = tx.send(());
            });

        quit.store(true, Ordering::Relaxed);
        watchdog.run_loop_left();

        assert!(
            rx.recv_timeout(Duration::from_millis(500)).is_err(),
            "the run loop answered, so the watchdog must not act"
        );
    }
}
