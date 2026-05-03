# Phase 09: `auto_resume_all_sessions` concurrency cap + staleness skip

> Maps to audit findings: P1-3, P2-3 (partial)

## Goal
Stop the thundering-herd at startup. `src/app/mod.rs:1380-1415` spawns a
PTY for every persisted session in a tight loop on the main thread. With
10+ sessions on a spot-VM resume this is multi-second freeze + a fork
bomb of Claude/Codex/Gemini processes. Add a configurable concurrency cap,
a staleness skip, and the missing tests called out in P2-3.

## Pre-conditions
- Phase 06 has extracted `patches/0002-auto-resume-on-start.diff`.
- Phase 00 baseline test green.

## Files to touch
- `src/config.rs` — add `auto_resume_concurrency` and `auto_resume_max_age_days`.
- `src/app/mod.rs` — modify `auto_resume_all_sessions`.
- `tests/auto_resume.rs` — new integration test.
- `patches/0002-auto-resume-on-start.diff` — regenerate.

## Steps
1. Config fields (defaults: concurrency 4, max-age 14):
   ```diff
   pub struct Defaults {
       pub auto_resume_on_start: bool,
   +   /// Cap on concurrent PTY spawns during auto-resume.
   +   pub auto_resume_concurrency: usize,
   +   /// Skip auto-resume for worktrees not modified within N days. 0 = no skip.
   +   pub auto_resume_max_age_days: u32,
       …
   }
   ```
   Provide defaults in `impl Default for Defaults`. Document in
   `install.sh`'s sed-patch block so regenerated config matches.
2. Refactor `auto_resume_all_sessions`: filter by max-age (compare
   worktree mtime via `std::fs::metadata().modified()` against
   `now - days*86400s`), chunk candidates by concurrency, and call
   `self.drain_worker_events_nonblocking()` between chunks so the UI
   doesn't freeze. Keep "serial with yield" for v1; future ticket can add
   true parallel spawn. The existing per-session spawn logic
   (`spawn_pty_for_session`, `mark_session_status`, error log+continue)
   is unchanged inside the inner loop.
3. `tests/auto_resume.rs`: synthetic session list with `auto_resume_on_start=true`,
   `auto_resume_concurrency=2`. Inject a fake `spawn_pty_for_session` via
   trait; assert filter logic skips missing/stale worktrees and chunks
   process in order.
4. Update README and `install.sh` sed block to document the two fields.
5. Regenerate `patches/0002-auto-resume-on-start.diff`.

## Validation
- `cargo test --test auto_resume` passes.
- `cargo clippy --all-targets -- -D warnings` clean.
- Manual: 6+ saved sessions, `auto_resume_concurrency=2`; log shows `cap 2`;
  `top` shows bounded spawn rate; UI is responsive.
- Touch worktree dirs older than 14 days → skipped with `max_age_days=14`.

## Acceptance criteria
- [x] `Defaults` has `auto_resume_concurrency` (default 4) + `auto_resume_max_age_days` (default 0; staleness skip is opt-in to preserve prior behavior).
- [x] Auto-resume processes candidates in chunks bounded by the cap; UI events drained between chunks via `App::drain_events`.
- [x] Stale worktrees skipped when `max_age_days > 0` (worktree mtime older than threshold; sessions whose mtime cannot be probed are also dropped when the cap is enabled).
- [x] Unit tests cover filter behavior (`already-spawned` skip, missing worktree skip, staleness skip with explicit fixture mtimes, unprobeable mtime).
- [ ] Patch `patches/0002-…` regenerated; `git apply --check` clean. *(Track C scope is Rust only per the audit01 split — `patches/` regeneration belongs to the wrapper-chain track and is deferred.)*

## References
- Audit P1-3, P2-3.
- Call site: `src/app/mod.rs:1380-1415`.
- `SystemTime::duration_since` and clock drift: https://doc.rust-lang.org/std/time/struct.SystemTime.html
