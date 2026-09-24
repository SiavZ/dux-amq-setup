# Core runtime and storage

This subsystem covers PTY ownership/shutdown, watch scheduling, Git and diff helpers, AMQ queue mechanics below the wrapper layer, polling workers, SQLite lifecycle, and session persistence. Findings use the frozen audit03 thresholds and baseline `3d52074`.

## P0-02 — Stale-inflight recovery can overwrite a newer queued message

**Impact.** A crash/restart recovery can silently replace a newer live message with the body of an older claimed message. That is direct message data loss in a normal supported lifecycle.

**Baseline evidence.** `3d52074 — src/amq_inject.rs:323-409 — reclaim_stale_inflight_with_max_age`: recovery reconstructs `<name>.msg` from `.inflight.<name>.msg` and calls `fs::rename(&inflight_path, &original_path)` at line 388 without checking that the destination remains absent. A producer can legitimately recreate the original filename while the prior claim is stranded. `3d52074 — src/amq_inject.rs:1063-1078 — reclaim_stale_inflight_continues_past_per_file_errors` explicitly creates both files and asserts that both reclaim, documenting that POSIX `rename` overwrites the destination.

**Adversarial verification.** The temporary-file producer discipline and age quarantine were checked. Neither prevents a fresh destination appearing between the original claim and a later process restart. On Unix, same-filesystem `rename` replaces the destination atomically; the test removes ambiguity about the intended platform behavior. The candidate therefore survives as P0, not a hypothetical race.

**Required proof direction.** Recovery must use no-replace semantics or quarantine the collision, preserve both bodies, and test the exact two-file state.

## P0-05 — Updating `.git/info/exclude` converts a read failure into destructive overwrite

**Impact.** Creating the worktree explorer link can erase a user's existing repository-local exclusions. A non-UTF-8 file is a deterministic reproduction; transient permission/I/O failures create the same destructive state transition if the later write succeeds.

**Baseline evidence.** `3d52074 — src/git.rs:328-370 — ensure_project_worktrees_link_ignored`: line 354 reads the sole exclude file with `fs::read_to_string(...).unwrap_or_default()`. Any read error, including `InvalidData` for non-UTF-8 bytes, becomes an empty string. Lines 362-369 then construct only the Dux stanza and overwrite the file with `fs::write`.

**Adversarial verification.** A missing file was considered: treating `NotFound` as empty is appropriate. The implementation does not distinguish `NotFound` from every other error, and Git's exclude file is not required to be UTF-8. No backup or byte-preserving path exists. Because the old contents are lost after a reachable normal operation, the frozen data-loss bar makes this P0.

**Required proof direction.** Preserve bytes, distinguish `NotFound`, fail closed on unreadable content, and atomically append/replace with a regression fixture containing invalid UTF-8.

## P0-06 — PTY teardown can block forever when a descendant keeps the slave open

**Impact.** Deleting, quitting, reconnecting, or otherwise dropping a session can hang the application indefinitely when a background descendant inherits the PTY slave.

**Baseline evidence.** `3d52074 — src/pty.rs:655-677 — impl Drop for PtyClient`: Drop kills only `self.child`, keeps `self.master` alive, and then synchronously joins the reader at lines 668-670. The comment itself says the reader receives EOF only once `self.master` is dropped, but that drop cannot occur until after the join returns. A surviving descendant retaining the slave also prevents terminal hangup/EOF. `3d52074 — src/pty.rs:1905-1922 — dropping_pty_client_returns_promptly` covers only a direct `sh -c 'sleep 5'` child, not a detached/background descendant.

**Adversarial verification.** The locked `portable-pty` behavior and process ownership were traced: the stored child handle kills the direct child, not an arbitrary descendant tree. Closing only the cloned reader is insufficient while the master/slave relationship remains live. The ordinary shell pattern `sleep 999 &` supplies a reachable counterexample. No join timeout exists. The finding survives as a normal lifecycle hang, P0.

**Required proof direction.** Establish bounded shutdown ordering (close/cancel reader before join and terminate the process group where supported) and retain a test with a background descendant plus a hard completion deadline.

## P0-07 — A finite watch capture can panic reset processing

**Impact.** Agent-controlled terminal output matching a configured wait-capture rule can panic the TUI instead of falling back to backoff.

**Baseline evidence.** `3d52074 — src/watch/reset_time.rs:23-66 — parse`: `InSeconds` checks the parsed `f64` before conversion, while `InMinutes` and `InHours` multiply only after the finite check. A large finite number can overflow the multiplication to infinity; `Duration::from_secs_f64` panics for non-finite/out-of-range input. Even a representable `Duration` can overflow `now_instant + duration`. `3d52074 — src/watch/engine.rs:415-452 — schedule_fire_at` passes the last regex capture from the live PTY snapshot directly to this parser and expects `None`, not unwind, on bad input.

**Adversarial verification.** Parsing rejects literal `inf`, NaN, negative values, and out-of-range Unix timestamps, but it does not validate post-multiplication values or use `Instant::checked_add`. Existing reset-time tests cover ordinary and syntactically invalid captures, not huge finite values. The output source is within documented threat-model T13 (agent output is untrusted), and a configured watch rule is a supported path. P0 is retained.

**Required proof direction.** Use checked unit conversion and `checked_add`, return `None` for every unrepresentable input, and add boundary tests for all formats.

## P1-14 — Configured periodic backups are never started

**Impact.** Operators are told they have periodic SQLite backups, but the only backup worker is dead code. Recovery therefore depends on an older or nonexistent `.bak` file.

**Baseline evidence.** `3d52074 — src/config.rs:1285-1295 — storage.backup_interval_minutes config entry` promises automatic Online Backup API copies and defaults to 30 minutes. `3d52074 — src/app/workers.rs:2012-2058 — spawn_backup_worker` implements the loop but is marked `#[allow(dead_code)]`; its comment says App wiring is deferred. The `App::new` worker startup region at `src/app/mod.rs:1485-1510` starts the other pollers and never calls it. A full-tree reference search found no call.

**Adversarial verification.** Startup integrity recovery and the manual backup API were checked; neither schedules recurring backups. The dead-code annotation and full-tree absence refute the possibility of indirect wiring. This is a promised durability rail with a direct missing edge, hence P1.

## P1-15 — Schema migrations are not atomic despite documentation saying they are

**Impact.** A failure between DDL statements or between DDL and `user_version` can leave a schema that retries cannot migrate, potentially preventing startup.

**Baseline evidence.** `3d52074 — src/storage.rs:44-70 — run_migrations` claims every `execute_batch` has an implicit transaction, but calls the migration SQL and `PRAGMA user_version` in separate unguarded batches. `3d52074 — src/storage/migrations/0004_session_sort_order.sql:7-18` is multi-statement DDL. `docs/operations/schema.md:43-50` repeats the atomicity claim. Locked dependency evidence is `Cargo.lock:2010-2012`, `rusqlite 0.39.0`. Its [`execute_batch` documentation](https://docs.rs/rusqlite/0.39.0/rusqlite/struct.Connection.html#method.execute_batch) executes the supplied batch; it does not promise an automatic transaction around SQL that lacks `BEGIN`/`COMMIT`.

**Adversarial verification.** SQLite's transactional DDL does not help when the caller does not start a transaction spanning all statements and the version bump. The migration files contain no transaction delimiters. Happy-path/idempotence tests do not inject a mid-migration failure. A partial `ALTER TABLE` followed by a retry can hit duplicate-column failure while `user_version` is still old. P1 is retained as a missing integrity rail.

## P1-17 — AMQ scanning caps an arbitrary directory subset before sorting

**Impact.** With more messages/receivers than the scan cap, filesystem enumeration order determines which messages are visible. Older messages or later receiver directories can starve indefinitely despite the advertised stable order.

**Baseline evidence.** `3d52074 — src/amq_inject.rs:182-248 — scan_queue_dir_limited`: the function stops pushing at `max_messages` on lines 222-243 and only sorts that arbitrary subset on line 248. The comment at lines 244-247 claims stable receiver/filename order, which cannot hold across capped scans because `read_dir` order is unspecified.

**Adversarial verification.** The filename convention makes lexical filename order correlate with arrival, but only after candidates have been selected. Repeated scans are not required to rotate directory enumeration, so eventual fairness is not guaranteed. The live cap is deliberately small to protect the TUI, making this a concrete backlog path rather than an extreme-memory concern. P1.

## P1-18 — Busy and missing-PTY deliveries never enter the timeout-warning state

**Impact.** Messages deferred because the matched agent is busy or has no PTY can remain queued without the promised timeout warning, leaving operators unable to distinguish delay from a stuck delivery path.

**Baseline evidence.** `3d52074 — src/app/inject_runtime.rs:580-685 — drain_amq_inject_queue` calls `maybe_warn_timeout` from multiple deferral branches. `3d52074 — src/app/inject_runtime.rs:1129-1151 — maybe_warn_timeout` computes `due` only when `amq_inject_last_warned` already contains the receiver (`last.is_some_and(...)`) and never initializes it. The map is initialized by the separate no-session warning path, so matched-but-busy and matched-without-PTY receivers remain `None` forever.

**Adversarial verification.** File-mtime expiry was checked and is a separate destructive/quarantine policy, not the documented soft warning. A receiver previously seen in the no-session branch may warn, but that incidental history does not repair the ordinary matched-session branches. Tests do not begin with a busy matched receiver and advance through the timeout. P1.

## P1-19 — The documented `u64::MAX` quiet-window escape hatch holds forever

**Impact.** An explicitly documented “always deliver” configuration produces the opposite behavior whenever a keystroke has been recorded: delivery is held for effectively the lifetime of the process.

**Baseline evidence.** `3d52074 — src/config.rs:503-516 — AmqInjectConfig.active_session_quiet_secs` documents `u64::MAX` as “always deliver, never skip.” `3d52074 — src/app/inject_runtime.rs:1239-1254 — should_hold_for_quiet_window` special-cases only zero; any nonzero value returns true while elapsed time is below the duration. Existing tests at `src/app/inject_runtime.rs:1498-1544` cover zero, normal idle, and the boundary, but not the documented maximum.

**Adversarial verification.** Conversion to `Duration::from_secs(u64::MAX)` itself is valid, so there is no earlier rejection that could reinterpret the setting. With `Some(last_keystroke)`, the comparison is predictably true. The docs are not merely imprecise; they prescribe a supported behavior-changing escape hatch that is inverted. P1.

## P1-22 — Diff generation turns read errors into invented empty files

**Impact.** A permission, object-database, or transient read failure is rendered as an actual content deletion/addition rather than an error, so users can act on a fabricated diff.

**Baseline evidence.** `3d52074 — src/diff.rs:42-72 — diff_file`: a missing HEAD result and every worktree `fs::read` error become empty content through `unwrap_or_default()`. `3d52074 — src/git.rs:796-810 — file_bytes_at_head` treats every nonzero `git cat-file` exit as “new/untracked,” not only a missing path.

**Adversarial verification.** Non-UTF-8 bytes are correctly diverted to the binary renderer by `is_renderable_text`, so that candidate branch was refuted. The surviving read-error paths discard the distinction before binary detection. A genuinely new or deleted empty side is valid, but that does not justify mapping all failures to the same state. The caller returns `Result`, so propagation is structurally available. P1 under the project's explicit error-surfacing tenet.

## P1-24 — Changed-file polling can fork once per untracked file every two seconds

**Impact.** A worktree containing many untracked files can cause hundreds or thousands of Git subprocesses per active polling interval, consuming CPU/process slots and degrading the UI.

**Baseline evidence.** `3d52074 — src/app/workers.rs:1197-1223 — spawn_changed_files_poller` invokes `git::changed_files` every two seconds while an agent is active. `3d52074 — src/git.rs:448-588 — changed_files / untracked_file_diff_stat` runs status, unstaged numstat, cached numstat, then an additional `git diff --no-index` for each untracked file absent from the first numstat. For 1,000 untracked files the static command count is approximately 1,003 every two seconds.

**Adversarial verification.** The fallback reads individual files only after the per-file subprocess fails; it does not remove the fork. Git's first numstat does not include untracked files, so the branch is expected, not exceptional. The poller is off the UI thread but still an unbounded hot-path resource amplifier. P1.

## P1-27 — Lifecycle persistence failures can orphan worktrees and report false success

**Impact.** Database failures after filesystem/branch mutations leave worktrees with no durable session identity, or leave in-memory state and success messages that revert on restart.

**Baseline evidence.** `3d52074 — src/app/workers.rs:8-26 — WorkerEvent::CreateAgentReady`: worktree and PTY creation have already succeeded when `upsert_session` is attempted; on failure the handler sets an error and `continue`s without removing or adopting the created worktree. The local client is dropped on that branch, but the earlier spawn-failure worktree cleanup at `src/app/workers.rs:1550-1599` does not cover this ready-event branch. Related lifecycle completion handlers silently discard persistence failures: `src/app/workers.rs:131-180` and `src/app/mod.rs:2754-2761,2898-3007,3029,3434` use `let _ = self.session_store.upsert_session(...)` after rename, sync, archive, provider, and mode state changes while reporting success.

**Adversarial verification.** The filesystem can be rediscovered manually, but no automatic rollback/adoption path was found and the sole database identity is absent. Later upserts might repair some in-memory changes, but they are not guaranteed before exit and do not excuse an explicit success message after a failed durable write. The cluster shares one lifecycle invariant: externally committed mutations must either persist or compensate. P1.

## Subsystem conclusion

The runtime has useful caps, recovery quarantine, state-machine types, WAL/backup primitives, and background workers, but several rails stop one edge short of the failure they claim to contain. The fix phase should preserve the existing simple design and close those exact edges with focused failure-injection/boundary tests; a broad storage or runtime rewrite is not justified by this audit.
