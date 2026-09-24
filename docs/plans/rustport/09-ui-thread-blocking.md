# Phase 09 — Eliminate blocking work on the UI thread

**Track:** C (correctness & resources) · **Parallel-safe with:** 08, 10, 11
**Depends on:** nothing · **Blocks:** nothing

## Goal

Honour the project's own constraint — *"All periodic or potentially-blocking work
(git commands, file I/O, network) must run in background workers, never on the main UI
thread. Even fast operations like `git symbolic-ref` should use workers"*
(`CLAUDE.md:62`) — which nine verified call paths currently violate, one of them able
to freeze the TUI indefinitely.

## Evidence

The run loop (`App::run`, `mod.rs:1671-1835`) calls `drain_events` →
`poll_pty_activity` → `tick_watch_engines` → `tick_amq_inject` →
`tick_orchestrator_watchdog` every iteration at ~10 Hz. Everything below runs inside
that loop.

| # | Site | Reached from | What blocks |
|---|---|---|---|
| **V1** | `peer.rs:730` ← `sessions.rs:1192` | `drain_events` (`workers.rs:354`) | **`flock(LockExclusive)` — blocking cross-process lock, no timeout, no `_Nonblock`.** Another AMQ process holding `meta/config.lock` freezes the TUI **indefinitely** |
| **V2** | `mod.rs:3538-3554` ← `tick_watch_engines` (`mod.rs:3494`) | **run loop, every iteration** | up to 3 `fs::read_dir` scans *per watched session* (`amq_activity.rs:41,53,62`) |
| **V3** | `workers.rs:1042` → `pty.rs:657` → `pty.rs:798` | `drain_events` | on **macOS**, forks `ps -p <pid> -o comm=` per companion terminal every ~2 s |
| **V4** | `mod.rs:3283, 3309` | `drain_events` epilogue | `PtyClient::drop` → `recv_timeout(250ms)` + `recv_timeout(1s)` + `join` — **up to 1.25 s stall per exiting session** (`pty.rs:694-710`) |
| **V5** | `git.rs:358` ← `sessions.rs:245` | `drain_events` (`workers.rs:27`) | `git rev-parse --git-path info/exclude` subprocess + symlink/dir writes |
| **V6** | `input.rs:3279` → `git.rs:188,199` | `handle_key` | 2 × `git rev-parse` subprocess **while the user presses Enter** |
| **V7** | `inject_runtime.rs:367-476` | `drain_events` (`workers.rs:790`) | up to 32 file reads + stats + renames per drain |
| **V8** | `workers.rs:179, 260, 1797` | `drain_events` | sqlite `upsert_*` **inside `for` loops**, unbounded in N |
| **V9** | `workers.rs:1849` → `sessions.rs:632` | `drain_events` (`:774`) | `fork` + `exec` of a provider CLI |

**Not a defect — do not "fix":** `mod.rs:1998` forks on the main thread deliberately
and documents why (macOS forbids forking from a worker thread). Preserve it.

CPU cost of V2 compounds: `tick_watch_engines` does two full-grid walks per watched
session per tick (`mod.rs:3487-3491`), and `busy_snapshot` is computed even though
`should_suppress_auto_clear` returns early for non-Worker sessions (`mod.rs:3516`).
With a `BTreeMap` probe per cell (`pty.rs:1008`), ten sessions is roughly
**400k probes/s and ~2k `String` allocations/s on the render thread.**

## In scope

Moving each path off the UI thread, plus the redraw-cadence work that makes the loop
cheap when idle.

## Out of scope

- Splitting `workers.rs` → Phase 16. This phase changes *where work runs*, not where
  code lives.
- Memory sizing → Phase 08 (V4 overlaps with the grid leak; coordinate).

## Work items

1. **V1 first — it is the only one that can hang forever.** Change `peer.rs:730` to a
   non-blocking `flock` with `FlockOperation::NonBlockingLockExclusive`, and on
   contention either retry on a worker with backoff or report a clear status-line
   message. `src/peer.rs` already uses `rustix::fs::flock`; keep the same crate so lock
   semantics stay identical to the bash wrappers, which lock the same
   `meta/config.lock`.
2. **V2 — stop scanning directories in the run loop.** Move the `amq_activity`
   `read_dir` work to a worker with a wall-clock interval, and cache the result. Also
   short-circuit `busy_snapshot` when `should_suppress_auto_clear` will return early
   (`mod.rs:3516`) — computing it first is pure waste.
3. **V2b — make the grid walks cheap.** Two full-grid walks per watched session per
   tick at ~10 Hz is the dominant CPU cost. Walk on a wall-clock interval, or maintain
   an incremental dirty marker updated as the PTY writes, rather than re-scanning.
4. **V3 — move the macOS process-name probe to a worker.** Forking `ps` from the run
   loop every ~2 s is both a stall and a fork storm with many terminals. Prefer the
   already-present `sysinfo` crate over shelling out at all.
5. **V4 — make `PtyClient::drop` non-blocking on the UI thread.** Hand the teardown to
   a reaper worker so an exiting session cannot stall the loop for 1.25 s. Coordinate
   with Phase 08 item 5, which fixes the leak on the same timeout path — **land them
   together**, since both touch `pty.rs:694-727`.
6. **V5, V6 — move `git rev-parse` calls to workers.** V6 is the worst
   user-perceptible one: two subprocesses spawn while the user is pressing Enter.
   `CLAUDE.md:62` names `git symbolic-ref` explicitly as the example of something that
   still belongs on a worker.
7. **V7 — bound the inject drain per tick.** Up to 32 file reads plus stats plus
   renames per drain, on the UI thread. Move to a worker, and honour the existing
   `MAX_INJECT_ACTIONS_PER_TICK` budget (`inject_runtime.rs:65`) as a real cap rather
   than a nominal one.
8. **V8 — batch the sqlite upserts.** `upsert_*` inside `for` loops, unbounded in N.
   Wrap in a single transaction on a worker.
9. **V9 — move provider `fork`+`exec` off the drain path**, except where the
   documented macOS constraint at `mod.rs:1998` requires main-thread forking; keep that
   path and its comment intact.
10. **Introduce a real dirty-flag redraw.** `terminal.draw` runs **unconditionally every
    iteration** (`mod.rs:1719`); `force_redraw` is not a dirty flag — it only adds a
    `terminal.clear()` (`mod.rs:1709-1717`). Track whether any state changed and skip
    the draw when nothing did. This is the single largest idle-CPU win and it makes the
    ~10 Hz cadence cheap rather than merely tolerable.
11. **Replace the two remaining tick-count timers with wall-clock**, per the project
    tenet *"Animations and periodic refreshes use wall-clock time, not tick counts"*:
    `workers.rs:1042` (`tick_count.is_multiple_of(20)`, comment claims "~every 2
    seconds") and `mod.rs:2210-2215` (`NUDGE_DURATION_TICKS = 15; // ~1.5s at
    100ms/tick`). **The correct pattern is directly adjacent** at `workers.rs:1050-1056`,
    which uses `last_refresh.elapsed() >= Duration::from_secs(2)`. The assumed 100 ms
    cadence is not guaranteed — interactive/PTY mode bypasses the `event::poll` governor
    entirely (`input.rs:1199`).
12. **Add a UI-thread stall guard in debug builds.** Assert (or log loudly) if a single
    run-loop iteration exceeds a threshold, naming the phase that overran. This is how
    the next V-class regression gets caught in development rather than in a bug report.

## Acceptance criteria

- [ ] No blocking `flock` reachable from the run loop; contention produces a status
      message, not a freeze. Verified by holding `meta/config.lock` from another process
      and confirming the TUI stays responsive.
- [ ] No `fs::read_dir`, subprocess spawn, or sqlite write remains on the run-loop path
      — verified by inspection of all nine sites plus a grep gate for
      `Command::new` / `read_dir` under `src/app/` run-loop functions.
- [ ] `PtyClient::drop` cannot stall the loop; exiting 8 sessions at once stays responsive.
- [ ] `tick_watch_engines` cost is independent of tick rate (wall-clock or incremental).
- [ ] `busy_snapshot` is not computed when `should_suppress_auto_clear` returns early.
- [ ] Redraw is skipped when no state changed; idle CPU measurably lower.
- [ ] Zero `tick_count`-derived timers remain (`grep -rn 'tick_count' src/` reviewed).
- [ ] `mod.rs:1998`'s deliberate main-thread fork is preserved with its comment.
- [ ] Debug-build stall guard present.
- [ ] Idle CPU measured before and after and recorded in the PR.

## Validation

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings

# V1 proof: hold the lock from another process, confirm the TUI does not freeze
flock -x /path/to/$AMQ_GLOBAL_ROOT/meta/config.lock sleep 60 &
cargo run   # navigate, switch sessions; must stay responsive

# idle CPU, before vs after
top -pid $(pgrep -n dux) -l 10 -stats cpu
```

## Risks

| Risk | Mitigation |
|---|---|
| Moving work to workers introduces new stale-result races | Phase 10 fixes the one that already exists (`ChangedFilesReady`); apply the same generation/worktree guard pattern to every newly-async path — `workers.rs:546-555` is the reference implementation |
| A dirty-flag redraw drops legitimate frames | Start conservative: mark dirty on any PTY byte, any key, any worker event, any timer tick that changes state. Optimise only with the Phase 06 snapshots green |
| Non-blocking flock changes registration semantics vs. the bash wrappers | The wrappers hard-fail when flock is unavailable (`ownership.bats:49`); match that contract — fail closed and visible, never silently unlocked |
| Splitting `pty.rs`/`workers.rs` later conflicts with these edits | Track C is deliberately scheduled before Track D lands; if they overlap, this phase wins and Track D rebases |

## References

- `artifacts/research-app-monoliths.md` §C11, §C9, §Run loop
- `artifacts/research-runtime-memory.md` §2.5 (CPU), §5.6 (thread/join/poison)
