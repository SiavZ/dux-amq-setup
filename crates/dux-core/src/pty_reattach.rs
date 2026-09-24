//! Re-attaching a PTY master that was inherited across an `exec`.
//!
//! # Why this exists
//!
//! A dux reload replaces the running binary with `exec` so a new build takes
//! effect without killing the agents. `exec` keeps the process's children and
//! its open file descriptors, so the agents survive and their PTY masters can
//! survive with them. What does NOT survive is everything that lived in the old
//! image's memory, including the `portable_pty::MasterPty` object wrapping each
//! master fd.
//!
//! The fd is still there. Only the Rust value describing it is gone. So the
//! replacement image needs a way to say "this number is a PTY master, treat it
//! as one", and that is what this module provides.
//!
//! # Why not rebuild the library's own type
//!
//! `portable_pty`'s `UnixMasterPty` has private fields and no constructor that
//! takes an existing descriptor, so an inherited fd cannot be turned back into
//! one from outside the crate. [`MasterPty`] is a trait, though, and `PtyClient`
//! already stores it as `Box<dyn MasterPty + Send>`, so implementing the trait
//! here is enough and nothing downstream has to know which kind it holds.
//!
//! # The one thing the caller must do first
//!
//! `portable_pty` sets `FD_CLOEXEC` on every master it creates, which means the
//! kernel closes it during `exec` and the agent becomes unreachable. Call
//! [`keep_open_across_exec`] on the fd BEFORE exec'ing, or there will be nothing
//! on the other side to re-attach to.

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

use anyhow::{Context, Result};
use portable_pty::{MasterPty, PtySize};

/// Clear `FD_CLOEXEC` so this descriptor survives an `exec` into a new binary.
///
/// `portable_pty` sets the flag when it opens a master (`unix.rs`, in its
/// `openpty`), which is the right default: an fd that leaks into an unrelated
/// child is a bug, and every normal spawn wants it closed. A reload is the one
/// case where the "child" is dux itself continuing, and the descriptor has to
/// outlive the image that opened it.
///
/// Verified rather than assumed: with the flag left set, the replacement image
/// finds the fd closed and cannot read or write the agent at all; with it
/// cleared, a write-then-read round trip through the inherited fd reaches the
/// original child process.
pub fn keep_open_across_exec(fd: RawFd) -> Result<()> {
    // SAFETY: `F_GETFD`/`F_SETFD` only read and write this descriptor's flags.
    // They do not transfer ownership, close it, or touch its offset, so an
    // invalid fd fails with EBADF rather than doing anything unsound.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("reading the descriptor flags of fd {fd}"));
    }
    let cleared = flags & !libc::FD_CLOEXEC;
    // SAFETY: as above; `cleared` is the value just read with one flag removed.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, cleared) } < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("clearing FD_CLOEXEC on fd {fd}"));
    }
    Ok(())
}

/// Whether `fd` would survive an `exec`, i.e. whether `FD_CLOEXEC` is clear.
///
/// Reads the kernel's answer rather than remembering what was set, so a handoff
/// can be checked immediately before the exec it depends on.
pub fn survives_exec(fd: RawFd) -> Result<bool> {
    // SAFETY: `F_GETFD` only reads this descriptor's flags.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("reading the descriptor flags of fd {fd}"));
    }
    Ok(flags & libc::FD_CLOEXEC == 0)
}

/// A PTY master that was inherited through an `exec` rather than opened here.
///
/// Owns the descriptor: dropping this closes it, exactly as dropping a freshly
/// opened master would. That matters because the replacement image is the only
/// owner left, so if this did not close the fd nothing ever would.
#[derive(Debug)]
pub struct ReattachedMaster {
    file: std::fs::File,
}

impl ReattachedMaster {
    /// Adopt `fd` as a PTY master.
    ///
    /// Checks that the descriptor really is a terminal before taking ownership,
    /// because the fd number comes from a handoff written by the previous image
    /// and a stale or wrong number would otherwise be adopted silently and fail
    /// much later as unexplained I/O errors against whatever else now holds that
    /// number.
    ///
    /// # Safety
    ///
    /// `fd` must be a descriptor this process owns and that nothing else will
    /// close; ownership moves into the returned value.
    pub unsafe fn adopt(fd: RawFd) -> Result<Self> {
        // SAFETY: `isatty` only inspects the descriptor.
        if unsafe { libc::isatty(fd) } != 1 {
            let err = std::io::Error::last_os_error();
            anyhow::bail!(
                "fd {fd} is not a terminal, so it is not a PTY master to re-attach: {err}"
            );
        }
        // SAFETY: the caller guarantees ownership of `fd`; `File` takes it over
        // and closes it on drop, which is the ownership this type documents.
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        Ok(Self { file })
    }

    /// The descriptor, still owned by this value.
    pub fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Give up ownership, returning the descriptor without closing it.
    ///
    /// The counterpart to [`adopt`](Self::adopt): used when a master is being
    /// handed on to yet another image rather than shut down.
    pub fn release(self) -> RawFd {
        self.file.into_raw_fd()
    }

    fn duplicate(&self) -> Result<std::fs::File> {
        self.file
            .try_clone()
            .context("cloning the re-attached PTY master descriptor")
    }
}

impl MasterPty for ReattachedMaster {
    fn resize(&self, size: PtySize) -> Result<()> {
        let winsize = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };
        // SAFETY: `TIOCSWINSZ` reads one `winsize` through the pointer and only
        // writes kernel-side state for this terminal.
        let rc = unsafe { libc::ioctl(self.raw_fd(), libc::TIOCSWINSZ, &winsize) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error())
                .context("TIOCSWINSZ on the re-attached PTY master");
        }
        Ok(())
    }

    fn get_size(&self) -> Result<PtySize> {
        let mut winsize = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: `TIOCGWINSZ` only fills the `winsize` this call owns.
        let rc = unsafe { libc::ioctl(self.raw_fd(), libc::TIOCGWINSZ, &mut winsize) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error())
                .context("TIOCGWINSZ on the re-attached PTY master");
        }
        Ok(PtySize {
            rows: winsize.ws_row,
            cols: winsize.ws_col,
            pixel_width: winsize.ws_xpixel,
            pixel_height: winsize.ws_ypixel,
        })
    }

    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>> {
        Ok(Box::new(self.duplicate()?))
    }

    fn take_writer(&self) -> Result<Box<dyn Write + Send>> {
        Ok(Box::new(self.duplicate()?))
    }

    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<libc::pid_t> {
        // SAFETY: `tcgetpgrp` only reads this terminal's foreground group.
        let pgrp = unsafe { libc::tcgetpgrp(self.raw_fd()) };
        // A PTY whose foreground group is gone answers 0 on macOS, which is not
        // a process group id; report "unknown" rather than a bogus one.
        (pgrp > 0).then_some(pgrp)
    }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<RawFd> {
        Some(self.raw_fd())
    }

    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        // The name of the SLAVE side, which is what a tty name means here. The
        // master has no path of its own to report.
        let name = unsafe { libc::ptsname(self.raw_fd()) };
        if name.is_null() {
            return None;
        }
        // SAFETY: `ptsname` returns a NUL-terminated string owned by libc that
        // stays valid until the next `ptsname` call on this thread; it is copied
        // out immediately here.
        let cstr = unsafe { std::ffi::CStr::from_ptr(name) };
        Some(std::path::PathBuf::from(
            cstr.to_string_lossy().into_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Open a real PTY pair and return `(master, slave)`, so these tests
    /// exercise the same kind of descriptor a reload inherits rather than a
    /// stand-in.
    ///
    /// The SLAVE is returned and kept alive by the caller on purpose. A master
    /// from `posix_openpt` whose slave was never opened rejects `TIOCSWINSZ`
    /// with `ENOTTY` on macOS, which is a property of a half-open pair rather
    /// than of the code under test: `openpty` (what `portable_pty` uses) opens
    /// both ends, and dux always has a child holding the slave. Dropping the
    /// slave here would test a state dux never reaches.
    fn open_pty_pair() -> (RawFd, RawFd) {
        let mut master: RawFd = -1;
        let mut slave: RawFd = -1;
        // SAFETY: `openpty` fills the two fds it is given and touches nothing
        // else; the null arguments opt out of the optional name and settings.
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(rc, 0, "openpty failed: {}", std::io::Error::last_os_error());
        (master, slave)
    }

    #[test]
    fn a_fresh_master_would_be_closed_by_exec_until_the_flag_is_cleared() {
        // The premise the whole reload rests on. A master opened the normal way
        // does NOT survive exec, which is why the handoff has to ask for it.
        let (fd, slave) = open_pty_pair();
        // Mirror what portable_pty does to every master it opens.
        // SAFETY: `F_SETFD` only sets this descriptor's flags.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            assert_eq!(libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC), 0);
        }
        assert!(
            !survives_exec(fd).unwrap(),
            "a master with FD_CLOEXEC set must be reported as not surviving exec"
        );

        keep_open_across_exec(fd).unwrap();
        assert!(
            survives_exec(fd).unwrap(),
            "after clearing the flag the master must be reported as surviving exec"
        );
        // SAFETY: nothing adopted these fds, so this scope still owns them.
        unsafe {
            libc::close(fd);
            libc::close(slave);
        }
    }

    #[test]
    fn an_adopted_master_reports_the_size_that_was_set_through_it() {
        // Proves the re-attached value is really driving the kernel's terminal
        // state, not just holding a number: the size read back is the one set.
        let (fd, slave) = open_pty_pair();
        // SAFETY: the fd was just opened here and is not owned elsewhere.
        let master = unsafe { ReattachedMaster::adopt(fd) }.unwrap();
        master
            .resize(PtySize {
                rows: 31,
                cols: 101,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let size = master.get_size().unwrap();
        assert_eq!((size.rows, size.cols), (31, 101));
        // SAFETY: the slave was never adopted, so this scope closes it.
        unsafe { libc::close(slave) };
    }

    #[test]
    fn an_adopted_master_still_drives_the_process_on_the_other_end() {
        // The whole point of the type: a descriptor that arrived from somewhere
        // else must be able to carry a real conversation with the slave side,
        // not merely answer ioctls about itself.
        use std::io::Write;

        let (fd, slave) = open_pty_pair();
        // SAFETY: freshly opened above and not owned elsewhere.
        let master = unsafe { ReattachedMaster::adopt(fd) }.unwrap();

        let mut writer = master.take_writer().unwrap();
        writer.write_all(b"hello through the master\n").unwrap();
        writer.flush().unwrap();

        // Read with a deadline rather than a bare blocking read. A writer that
        // silently went nowhere (the regression this guards) would otherwise
        // leave the read waiting forever and hang the whole suite instead of
        // failing it. `O_NONBLOCK` plus a bounded poll turns that into a
        // verdict.
        // SAFETY: `F_SETFL` only changes this descriptor's status flags.
        unsafe {
            let flags = libc::fcntl(slave, libc::F_GETFL);
            assert_eq!(
                libc::fcntl(slave, libc::F_SETFL, flags | libc::O_NONBLOCK),
                0
            );
        }
        let mut buf = [0u8; 64];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got = String::new();
        while std::time::Instant::now() < deadline {
            // SAFETY: reads at most `buf.len()` bytes into a buffer this scope owns.
            let n = unsafe { libc::read(slave, buf.as_mut_ptr().cast(), buf.len()) };
            if n > 0 {
                got.push_str(&String::from_utf8_lossy(&buf[..n as usize]));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // SAFETY: the slave was never adopted, so this scope closes it.
        unsafe { libc::close(slave) };

        assert!(
            got.contains("hello through the master"),
            "the slave must receive what was written to the re-attached master, got {got:?}"
        );
    }

    #[test]
    fn adopting_a_descriptor_that_is_not_a_terminal_is_refused() {
        // The fd number arrives from a handoff file written by the previous
        // image. A stale number must fail here, where it names the problem,
        // rather than being adopted and failing later as mystery I/O errors.
        let file = tempfile::NamedTempFile::new().unwrap();
        let fd = file.as_file().as_raw_fd();
        // SAFETY: the fd is valid and owned by `file`; adoption is expected to
        // fail before taking ownership, so `file` still closes it.
        let err = unsafe { ReattachedMaster::adopt(fd) }.unwrap_err();
        assert!(
            err.to_string().contains("not a terminal"),
            "the refusal must say what was wrong, got: {err}"
        );
    }

    #[test]
    fn released_descriptor_stays_open_for_the_next_image() {
        // A master may be handed on again. Releasing must not close it, or the
        // second reload in a row would hand over a dead number.
        let (fd, slave) = open_pty_pair();
        // SAFETY: freshly opened above.
        let master = unsafe { ReattachedMaster::adopt(fd) }.unwrap();
        let released = master.release();
        assert_eq!(released, fd);
        assert!(
            survives_exec(released).is_ok(),
            "a released descriptor must still be a live fd"
        );
        // SAFETY: ownership came back out of `release`, so this scope closes it.
        unsafe {
            libc::close(released);
            libc::close(slave);
        }
    }
}
