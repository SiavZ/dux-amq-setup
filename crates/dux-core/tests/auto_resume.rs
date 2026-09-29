//! Integration tests for the bounded auto-resume primitives (fork
//! tests/auto_resume.rs, 07d9b0ba), driven without real PTYs:
//!
//! 1. The peak concurrency never exceeds `cfg.concurrency`.
//! 2. `is_stale` reports stale worktrees as stale, so the startup candidate
//!    filter skips them.
//! 3. The stagger introduces a measurable minimum gap between dispatches.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use dux_core::auto_resume::{is_stale, run_scheduler};
use dux_core::config::AutoResumeConfig;

/// Set the modification time of `path` to `seconds` seconds before now.
fn set_mtime_seconds_ago(path: &Path, seconds: u64) {
    let target = SystemTime::now()
        .checked_sub(Duration::from_secs(seconds))
        .expect("test clock underflow");
    std::fs::File::open(path)
        .expect("open for mtime")
        .set_modified(target)
        .expect("set_modified");
}

#[test]
fn at_most_concurrency_workers_at_once() {
    // 12 jobs through a 3-permit semaphore; peak in-flight count must
    // never exceed 3. Each worker holds the permit for ~25 ms, long
    // enough that the next dispatch competes for a slot rather than
    // racing through.
    let cfg = AutoResumeConfig {
        concurrency: 3,
        stale_days: 0,
        stagger_ms: 0,
    };
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let in_flight_clone = Arc::clone(&in_flight);
    let peak_clone = Arc::clone(&peak);
    let jobs: Vec<usize> = (0..12).collect();

    run_scheduler(jobs, &cfg, move |_job| {
        let now = in_flight_clone.fetch_add(1, Ordering::SeqCst) + 1;
        peak_clone.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(25));
        in_flight_clone.fetch_sub(1, Ordering::SeqCst);
    });

    let observed_peak = peak.load(Ordering::SeqCst);
    assert!(
        (1..=3).contains(&observed_peak),
        "peak in-flight workers should be in [1, 3], got {observed_peak}"
    );
    assert_eq!(in_flight.load(Ordering::SeqCst), 0);
}

#[test]
fn stale_worktree_is_skipped() {
    // is_stale is the gate the production scheduler runs before
    // building its candidate list.
    let tmp = tempfile::tempdir().unwrap();
    let stale_dir = tmp.path().join("old-worktree");
    std::fs::create_dir(&stale_dir).unwrap();
    // 60 days ago, well past the default 30-day cutoff.
    set_mtime_seconds_ago(&stale_dir, 60 * 86_400);

    let fresh_dir = tmp.path().join("fresh-worktree");
    std::fs::create_dir(&fresh_dir).unwrap();

    assert!(
        is_stale(&stale_dir, 30),
        "60-day-old worktree should be stale at threshold=30",
    );
    assert!(
        !is_stale(&fresh_dir, 30),
        "newly-created worktree should not be stale",
    );
    assert!(
        !is_stale(&stale_dir, 0),
        "stale_days=0 must disable the filter",
    );
}

#[test]
fn stagger_introduces_minimum_delay() {
    // Run two scheduler invocations with the same job count: one with no
    // stagger, one with a 100ms stagger. The staggered run must take
    // measurably longer, which proves the gating is active without
    // pinning us to fragile per-pair timestamp comparisons (worker
    // threads can record their `Instant::now()` in non-dispatch order
    // because the spawn closure runs concurrently with the next loop
    // iteration).
    let job_count = 5;
    let stagger_ms = 100u64;
    let no_stagger = AutoResumeConfig {
        concurrency: 8,
        stale_days: 0,
        stagger_ms: 0,
    };
    let with_stagger = AutoResumeConfig {
        concurrency: 8,
        stale_days: 0,
        stagger_ms,
    };
    let count_unstaggered = Arc::new(AtomicUsize::new(0));
    let count_staggered = Arc::new(AtomicUsize::new(0));

    let cu = Arc::clone(&count_unstaggered);
    let t0 = Instant::now();
    run_scheduler((0..job_count).collect::<Vec<_>>(), &no_stagger, move |_| {
        cu.fetch_add(1, Ordering::SeqCst);
    });
    let unstaggered = t0.elapsed();

    let cs = Arc::clone(&count_staggered);
    let t1 = Instant::now();
    run_scheduler(
        (0..job_count).collect::<Vec<_>>(),
        &with_stagger,
        move |_| {
            cs.fetch_add(1, Ordering::SeqCst);
        },
    );
    let staggered = t1.elapsed();

    assert_eq!(count_unstaggered.load(Ordering::SeqCst), job_count);
    assert_eq!(count_staggered.load(Ordering::SeqCst), job_count);

    // (job_count - 1) gaps must fit between the (job_count) dispatches.
    // Allow a generous 50% slack so a single-core CI host with jitter
    // doesn't flake; the point is to prove the gating is on, not to
    // measure the exact stagger.
    let expected_min = Duration::from_millis(stagger_ms * (job_count as u64 - 1) / 2);
    assert!(
        staggered >= expected_min,
        "staggered run was unexpectedly fast: {staggered:?} (expected >= {expected_min:?})",
    );
    assert!(
        staggered > unstaggered + Duration::from_millis(50),
        "staggered run ({staggered:?}) should be visibly slower than unstaggered ({unstaggered:?})",
    );
}
