//! Periodic backup of the session database (port of fork 10d2266d, P1-W).
//!
//! Every `[storage].backup_interval_minutes` a loop worker copies
//! `sessions.sqlite3` to `sessions.sqlite3.bak` with SQLite's online backup, so
//! the integrity check on open has a recent copy to point a user at. The worker
//! opens its own connection each time rather than borrowing the engine's: WAL
//! lets a reader copy while the engine writes, and nothing on the engine thread
//! waits for the copy.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use super::Engine;
use super::spawn_worker::{LoopControl, LoopWorkerSpec};

/// Where the backup lands: beside the database, named as the integrity-check
/// error tells the user to look.
pub fn backup_path(sessions_db_path: &Path) -> PathBuf {
    let mut name = sessions_db_path.as_os_str().to_os_string();
    name.push(".bak");
    PathBuf::from(name)
}

/// One backup: open the live database read-only (no migration, no WAL
/// switch) and copy it. Returns where it went.
pub fn backup_once(sessions_db_path: &Path) -> anyhow::Result<PathBuf> {
    let dst = backup_path(sessions_db_path);
    crate::storage::SessionStore::open_read_only(sessions_db_path)?.backup_to(&dst)?;
    Ok(dst)
}

/// The interval as a duration, or `None` when backups are off.
pub fn backup_interval(minutes: u32) -> Option<Duration> {
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
}

impl Engine {
    /// Start the periodic backup worker, once per engine (the surface flip
    /// re-calls the global spawns). `backup_interval_minutes = 0` starts
    /// nothing. The interval is read at startup, as the config comment says.
    pub fn spawn_backup_worker(&self) {
        let Some(interval) = backup_interval(self.config.storage.backup_interval_minutes) else {
            crate::logger::info("[storage] periodic backup disabled (backup_interval_minutes = 0)");
            return;
        };
        self.spawn_backup_worker_every(interval);
    }

    fn spawn_backup_worker_every(&self, interval: Duration) {
        let started = Arc::clone(&self.limits.backup_worker_started);
        if started.swap(true, Ordering::Relaxed) {
            return;
        }
        let db = self.paths.sessions_db_path.clone();
        let ok = self.spawn_loop_worker(
            LoopWorkerSpec {
                label: "storage-backup".into(),
                feature: "session database backups".into(),
                remedy: crate::poller_status::REMEDY_RESTART_DUX.into(),
            },
            move |_tx| {
                thread::sleep(interval);
                if db.exists() {
                    match backup_once(&db) {
                        Ok(dst) => crate::logger::debug(&format!(
                            "[storage] backup ok -> {}",
                            dst.display()
                        )),
                        // Logged, not surfaced: a transient failure is retried
                        // at the next interval, and the previous copy stays.
                        Err(err) => crate::logger::warn(&format!(
                            "[storage] backup of {} failed: {err:#}",
                            db.display()
                        )),
                    }
                }
                LoopControl::Continue
            },
        );
        if !ok {
            started.store(false, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_session, test_engine};

    #[test]
    fn backup_worker_runs_on_schedule_and_zero_disables() {
        assert_eq!(backup_interval(0), None);
        assert_eq!(backup_interval(30), Some(Duration::from_secs(1800)));

        let (mut engine, _tmp) = test_engine();
        let bak = backup_path(&engine.paths.sessions_db_path);
        engine
            .session_store
            .create_session(&sample_session("s1", "p1", "feat"))
            .unwrap();

        engine.config.storage.backup_interval_minutes = 0;
        engine.spawn_backup_worker();
        assert!(!engine.limits.backup_worker_started.load(Ordering::Relaxed));

        engine.spawn_backup_worker_every(Duration::from_millis(20));
        engine.spawn_backup_worker_every(Duration::from_millis(20)); // idempotent
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !bak.exists() && std::time::Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(bak.exists(), "the worker never wrote {}", bak.display());
        let restored = crate::storage::SessionStore::open_read_only(&bak).unwrap();
        assert_eq!(restored.load_sessions().unwrap()[0].id, "s1");
    }

    #[test]
    fn backup_to_produces_valid_db() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("sessions.sqlite3");
        drop(crate::storage::SessionStore::open(&src).unwrap());
        let dst = backup_once(&src).expect("backup succeeds on a fresh database");
        assert_eq!(dst, tmp.path().join("sessions.sqlite3.bak"));
        assert!(std::fs::metadata(&dst).unwrap().len() > 0);
        // The copy opens through the normal hardened path (integrity check,
        // migration) just as a restore would.
        crate::storage::SessionStore::open(&dst).expect("the backup opens");
    }

    /// Both surfaces start it with the other global workers.
    #[test]
    fn app_run_wires_the_periodic_backup_worker() {
        let tui = include_str!("../../../dux-tui/src/app/mod.rs");
        let web = include_str!("../../../dux-web/src/engine_actor.rs");
        assert!(tui.contains("self.engine.spawn_backup_worker();"));
        assert!(web.contains("engine.spawn_backup_worker();"));
    }
}
