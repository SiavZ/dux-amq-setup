//! Throttled startup relaunch of persisted agents (fork 478c9e3c, 07d9b0ba).
//!
//! Relaunching every saved agent in one tight loop starts that many provider
//! processes and TLS handshakes at the same instant. With ~50 agents on a
//! resumed VM that is a fork bomb, and several provider APIs rate-limit the
//! handshake burst. `[auto_resume]` bounds it: at most `concurrency` startup
//! launches in flight, at least `stagger_ms` between two dispatches, and agents
//! whose directory went untouched for `stale_days` are skipped.
//!
//! Two pieces live here:
//!
//! * [`StartupLaunchQueue`], the engine-side throttle. Upstream already spawns
//!   every PTY on a worker thread (`Command::DispatchAgentLaunch`), so the
//!   fork's reason for marshalling the fork back to the main thread (macOS
//!   `fork()` safety, 0a2c0371) is met by the existing chokepoint. What the
//!   engine still needs is a queue it drains each tick, releasing a launch only
//!   when a slot is free and the stagger has elapsed.
//! * [`run_scheduler`] and [`is_stale`], the fork's generic primitives, kept for
//!   callers that fan out blocking work on threads.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::config::AutoResumeConfig;
use crate::ids::TabId;

/// Returns `true` when `worktree` was last modified more than `days` days
/// ago. `days == 0` disables the check; missing or unreadable metadata is
/// treated as "not stale" so the session still gets a chance to resume, and
/// the spawn itself surfaces the real error if the path is gone.
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

/// A startup relaunch waiting for a throttle slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedStartupLaunch {
    pub session_id: String,
}

/// The engine-side startup throttle. Pure bookkeeping: the engine asks it which
/// queued launch may go now and tells it which tabs those launches occupy, and
/// it never touches a PTY itself.
#[derive(Debug, Default)]
pub struct StartupLaunchQueue {
    pending: VecDeque<QueuedStartupLaunch>,
    /// Tabs whose startup launch was dispatched and has not resolved yet. A
    /// launch leaves this set when the engine's in-flight guard for its tab
    /// clears, whether it succeeded or failed.
    in_flight: Vec<TabId>,
    last_dispatch: Option<Instant>,
    /// Set while the one-time history recovery runs and a queued agent needs
    /// its result (a shared agent, which has no resume-latest fallback).
    held: bool,
}

impl StartupLaunchQueue {
    pub fn enqueue(&mut self, launch: QueuedStartupLaunch) {
        if !self
            .pending
            .iter()
            .any(|queued| queued.session_id == launch.session_id)
        {
            self.pending.push_back(launch);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty() && self.in_flight.is_empty()
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Hold every queued launch until [`Self::release`].
    pub fn hold(&mut self) {
        self.held = true;
    }

    pub fn release(&mut self) {
        self.held = false;
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Forget dispatched launches that `still_launching` says have resolved.
    pub fn settle(&mut self, still_launching: impl Fn(&TabId) -> bool) {
        self.in_flight.retain(|tab| still_launching(tab));
    }

    /// The next launch allowed to go at `now`, if any. The caller dispatches it
    /// and reports its tab through [`Self::dispatched`].
    pub fn next_ready(
        &mut self,
        cfg: &AutoResumeConfig,
        now: Instant,
    ) -> Option<QueuedStartupLaunch> {
        if self.held || self.pending.is_empty() || self.in_flight.len() >= cfg.concurrency.max(1) {
            return None;
        }
        if let Some(last) = self.last_dispatch
            && now.saturating_duration_since(last) < Duration::from_millis(cfg.stagger_ms)
        {
            return None;
        }
        self.pending.pop_front()
    }

    /// Record that a launch taken from [`Self::next_ready`] was dispatched at
    /// `now` and occupies `tab` until it resolves. `None` means the chokepoint
    /// refused it, which still counts toward the stagger but holds no slot.
    pub fn dispatched(&mut self, tab: Option<TabId>, now: Instant) {
        self.last_dispatch = Some(now);
        if let Some(tab) = tab {
            self.in_flight.push(tab);
        }
    }
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
/// AND a permit is available. Returns once every spawned worker has
/// finished (joined).
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
    let mut last_dispatch: Option<Instant> = None;
    for job in jobs {
        // Stagger: enforce a minimum gap between successive dispatches so
        // N handshakes do not fire in the same millisecond even when permits
        // are immediately available.
        if let Some(prev) = last_dispatch {
            let elapsed = prev.elapsed();
            if elapsed < stagger {
                thread::sleep(stagger - elapsed);
            }
        }
        let permit = acquire(&permits);
        last_dispatch = Some(Instant::now());
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

    fn queued(id: &str) -> QueuedStartupLaunch {
        QueuedStartupLaunch {
            session_id: id.to_string(),
        }
    }

    /// The queue never lets more than `concurrency` launches be in flight, and
    /// a slot frees only once its launch resolved.
    #[test]
    fn startup_queue_holds_at_most_concurrency_launches_in_flight() {
        let cfg = AutoResumeConfig {
            concurrency: 2,
            stale_days: 0,
            stagger_ms: 0,
        };
        let mut queue = StartupLaunchQueue::default();
        for id in ["a", "b", "c"] {
            queue.enqueue(queued(id));
        }
        let now = Instant::now();
        let first = queue.next_ready(&cfg, now).unwrap();
        queue.dispatched(Some(TabId::new("tab-a")), now);
        let second = queue.next_ready(&cfg, now).unwrap();
        queue.dispatched(Some(TabId::new("tab-b")), now);
        assert_eq!((first, second), (queued("a"), queued("b")));
        assert_eq!(queue.next_ready(&cfg, now), None, "both slots are taken");

        queue.settle(|tab| tab.as_str() != "tab-a");
        assert_eq!(queue.next_ready(&cfg, now), Some(queued("c")));
    }

    /// Two dispatches are never closer than `stagger_ms`, even with free slots.
    #[test]
    fn startup_queue_spaces_dispatches_by_the_stagger() {
        let cfg = AutoResumeConfig {
            concurrency: 8,
            stale_days: 0,
            stagger_ms: 100,
        };
        let mut queue = StartupLaunchQueue::default();
        queue.enqueue(queued("a"));
        queue.enqueue(queued("b"));
        let t0 = Instant::now();
        assert!(queue.next_ready(&cfg, t0).is_some());
        queue.dispatched(None, t0);
        assert_eq!(queue.next_ready(&cfg, t0 + Duration::from_millis(99)), None);
        assert_eq!(
            queue.next_ready(&cfg, t0 + Duration::from_millis(100)),
            Some(queued("b"))
        );
    }

    /// Concurrency 0 means 1, not "never launch anything".
    #[test]
    fn startup_queue_treats_zero_concurrency_as_one() {
        let cfg = AutoResumeConfig {
            concurrency: 0,
            stale_days: 0,
            stagger_ms: 0,
        };
        let mut queue = StartupLaunchQueue::default();
        queue.enqueue(queued("a"));
        queue.enqueue(queued("a"));
        assert_eq!(queue.pending_len(), 1, "a session is queued once");
        assert!(queue.next_ready(&cfg, Instant::now()).is_some());
    }
}
