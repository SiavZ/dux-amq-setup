//! Bounded scheduler for auto-resume of persisted agent sessions.
//!
//! `App::auto_resume_all_sessions` previously walked every saved session
//! and spawned a PTY synchronously in a tight loop. With ~50 sessions on a
//! spot-VM resume that becomes a fork-bomb of provider processes, plus a
//! synchronous burst of provider TLS handshakes that several APIs
//! rate-limit. This module provides the pure pieces of the fix:
//!
//! * [`is_stale`] — answer "should we even consider this worktree?".
//! * [`run_scheduler`] — fan out a list of jobs across at most
//!   `concurrency` worker threads, with a `stagger_ms` minimum gap
//!   between successive spawn attempts.
//!
//! Both are intentionally generic over the spawn closure so the
//! production call site can ship a real PTY result back to the UI thread
//! while integration tests can drive the same scheduler with a mock that
//! records concurrency peaks and timestamps.
//!
//! The scheduler uses a `Mutex<usize> + Condvar` mini-semaphore — no new
//! crate dependencies. Permits are released through an RAII guard so a
//! panic inside the spawn closure can't leak the slot and stall the queue.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use crate::config::AutoResumeConfig;

/// Returns `true` when `worktree` was last modified more than `days` days
/// ago. `days == 0` disables the check; missing or unreadable metadata is
/// treated as "not stale" so the session still gets a chance to resume —
/// the spawn itself will surface the real error if the path is gone.
pub fn is_stale(worktree: &Path, days: u32) -> bool {
    if days == 0 {
        return false;
    }
    let Ok(meta) = std::fs::metadata(worktree) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let age = SystemTime::now().duration_since(mtime).unwrap_or_default();
    age.as_secs() > u64::from(days) * 86_400
}

/// RAII permit for the mini-semaphore. Releases the slot when dropped so
/// a panic inside the spawn closure can't leak permits.
struct Permit {
    inner: Arc<(Mutex<usize>, Condvar)>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.inner;
        let mut available = lock.lock().expect("auto-resume permit mutex poisoned");
        *available += 1;
        cvar.notify_one();
    }
}

fn acquire(inner: &Arc<(Mutex<usize>, Condvar)>) -> Permit {
    let (lock, cvar) = &**inner;
    let mut available = lock.lock().expect("auto-resume permit mutex poisoned");
    while *available == 0 {
        // Re-check in a `while` because `Condvar::wait` may spuriously wake.
        available = cvar
            .wait(available)
            .expect("auto-resume permit condvar poisoned");
    }
    *available -= 1;
    Permit {
        inner: Arc::clone(inner),
    }
}

/// Fan out a list of jobs across at most `cfg.concurrency` worker threads.
///
/// Each call to `spawn_one(job)` runs on its own thread, holding a permit
/// for the entire call. The next job is dispatched only after at least
/// `cfg.stagger_ms` milliseconds have elapsed since the previous dispatch
/// **and** a permit is available. Returns once every spawned worker has
/// finished (joined), so callers in tests get deterministic ordering and
/// the production caller can rely on the scheduler thread terminating.
///
/// `spawn_one` must be `Send + Sync + 'static` because it is shared
/// across worker threads. Each `T` must also be `Send + 'static` for the
/// same reason.
pub fn run_scheduler<T, F>(jobs: Vec<T>, cfg: &AutoResumeConfig, spawn_one: F)
where
    T: Send + 'static,
    F: Fn(T) + Send + Sync + 'static,
{
    if jobs.is_empty() {
        return;
    }
    let concurrency = cfg.concurrency.max(1);
    let stagger = Duration::from_millis(cfg.stagger_ms);
    let permits = Arc::new((Mutex::new(concurrency), Condvar::new()));
    let spawn_one = Arc::new(spawn_one);

    let mut handles = Vec::with_capacity(jobs.len());
    let mut last_dispatch: Option<std::time::Instant> = None;
    for job in jobs {
        // Stagger: enforce a minimum gap between successive dispatches so
        // we don't fire N TLS handshakes in the same millisecond even
        // when permits are immediately available.
        if let Some(prev) = last_dispatch {
            let elapsed = prev.elapsed();
            if elapsed < stagger {
                thread::sleep(stagger - elapsed);
            }
        }
        let permit = acquire(&permits);
        last_dispatch = Some(std::time::Instant::now());
        let spawn_one = Arc::clone(&spawn_one);
        let handle = thread::Builder::new()
            .name("auto-resume-spawn".into())
            .spawn(move || {
                // Permit is released when this guard drops, even on panic.
                let _permit = permit;
                spawn_one(job);
            })
            .expect("auto-resume worker thread spawn failed");
        handles.push(handle);
    }

    for h in handles {
        let _ = h.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stale_zero_days_disables_filter() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_stale(tmp.path(), 0));
    }

    #[test]
    fn is_stale_missing_path_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        assert!(!is_stale(&missing, 30));
    }
}
