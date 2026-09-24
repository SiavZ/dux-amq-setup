# Runtime Subsystems Research Brief — dux

**Scope.** PTY / terminal emulation, storage / model / migrations, git / filesystem watching / diff, peer + AMQ messaging, auto-resume, purge — and the application's actual memory behaviour.

**Motivation.** The operator runs many agents in parallel on one memory-constrained machine. Memory quantification is the headline deliverable.

**Method.** Four sub-agents read the assigned subsystems in full and reported with citations; the peer/AMQ/inject/resume/purge Rust and the entire `dux-amq/` bash layer were read directly, and every load-bearing claim was re-verified firsthand. Claims marked **✓** were verified directly against the source or by execution, not relayed.

**Baseline.** 69,515 lines of Rust under `src/`. The previous audit (`docs/audits/audit03/`) baselined at commit `3d52074` (2026-07-10); HEAD `562419e` (2026-08-31) is **82 commits and +11,306 lines later** — a 19.4% growth in seven weeks. Most `audit03` findings have since been fixed; the defects listed here are new, newly introduced, or explicitly deferred by that audit.

**Test reality.** `cargo test` exits 0 with **1,139 distinct tests executed** on macOS (1,056 unit + 83 integration), in ~16 s. The commonly quoted 1,149 figure double-counts the lib suite, which reruns under the `main.rs` bin target. A further 123 `bats` tests cover the bash layer but run in a **separate CI job** (`.github/workflows/overlay-ci.yml:38`), never under `cargo test`. `tests/git_portability.rs` is `#![cfg(target_os = "linux")]` and reports `running 0 tests` on macOS.

---

## 1. Verified structure

### 1.1 PTY and terminal emulation — `src/pty.rs` (2,161 lines), `src/raw_input.rs` (763)

**Spawn.** `PtyClient::spawn_with_env` (`pty.rs:196-222`) opens a PTY via `portable-pty`'s `NativePtySystem::openpty`, builds a `CommandBuilder` with args and cwd, applies per-session env vars, then calls `apply_terminal_env` **last** so the terminal-protocol variables cannot be overridden by operator config (`pty.rs:186-190` documents this ordering intent). After `spawn_command` it drops the slave handle so reads on the master receive EOF when the child exits (`pty.rs:216-218`).

`apply_terminal_env_from_parent` (`pty.rs:1297-1319`) sets `TERM` (normalised; `dumb` and empty both become `xterm-256color`, `pty.rs:1321-1332`), `COLORTERM` when the parent has a non-empty value, and unconditionally `DUX_PANE=1` and `DUX_PID=<pid>` (`pty.rs:1225-1226`) ✓.

**Thread model.** One reader thread per PTY (`pty.rs:255-277`), reading into a stack-allocated `[0u8; 4096]` (`pty.rs:308`). The writer is an `Arc<Mutex<Box<dyn Write + Send>>>` (`pty.rs:229-231`); the terminal is an `Arc<Mutex<TerminalState>>` (`pty.rs:84`). Coordination uses `AtomicBool` flags — `exited`, `has_output`, `dirty`, `received_data`, `scroll_paused` (`pty.rs:86-90`).

**Teardown.** `impl Drop for PtyClient` (`pty.rs:662-727`): `try_wait` to detect an already-reaped child, then `SIGHUP` to the child's **process group**, a 250 ms grace, then `SIGKILL` to the group plus `child.kill()`, then up to 1 s waiting for the reader thread to join. On timeout the thread is **detached** rather than joined (`pty.rs:717-727`, logged as "PTY reader did not stop before shutdown deadline; detaching thread"). See defect D-P1.

**alacritty embedding.** `alacritty_terminal` `0.26.0` (`Cargo.toml`), resolved identically in `Cargo.lock:21-23`; 0.26.0 is the current release (published 2026-04-06), so the pin is not stale. `TerminalState::new` constructs `Config { scrolling_history: scrollback_lines, .. }` (`pty.rs:840-843`), threaded from `config.ui.agent_scrollback_lines`, default **10,000** (`config.rs:820`) ✓. Grid resize goes through `Term::resize` in `PtyClient::resize` (`pty.rs:546`).

**Ownership after audit02 P1-Z phase 2.** There is no `providers: HashMap<String, PtyClient>` on `App` any more (`app/state/runtime.rs:25-28` documents its removal). The PTY handle lives inside the session's own lifecycle enum — `SessionState::Live` / `SessionState::Detached` (`model.rs:126-150`) — and is reached via `App::find_pty_handle`.

**Raw input.** `src/raw_input.rs` classifies and splits terminal input; production code is only lines 1–264, the remainder being tests. The OSC/CSI splitter returns unconsumed bytes as a remainder (`raw_input.rs:208-231`), which is what makes the accumulating buffer unbounded (D-P3).

### 1.2 Storage and model — `src/storage.rs` (1,485), `src/model.rs` (887)

**Schema version.** `user_version = 6` (`storage.rs:150-153`, asserted again at `storage.rs:410-413`).

**Migration runner.** Migrations are a compile-time table of `include_str!`ed SQL files (`storage.rs:129-154`). Each is applied inside **one transaction that also carries the `user_version` bump** (`storage.rs:229-244`) — this closes `audit03` P1-15, which found the DDL and the version bump in separate unguarded batches. Migration 5 is a full table rebuild with a Rust-driven agent-handle backfill split at an in-file marker (`storage.rs:264-350`) and a `foreign_key_check` assertion afterwards. `tests/storage_migrations.rs` proves migration 5 rolls back atomically.

**Connection and PRAGMAs.** A single `Arc<Mutex<Connection>>` (`storage.rs:386-389`). `open_connection` (`storage.rs:886-897`) ✓ executes exactly: `journal_mode = WAL`, `synchronous = NORMAL`, `temp_store = MEMORY`, `mmap_size = 134217728` (128 MiB), `wal_autocheckpoint = 1000`, `foreign_keys = ON`. **There is no `busy_timeout` PRAGMA anywhere in `src/`** ✓. However `rusqlite 0.39.0` calls `sqlite3_busy_timeout(db, 5000)` unconditionally in its open path (`~/.cargo/registry/.../rusqlite-0.39.0/src/inner_connection.rs:118`) ✓, so a **5-second retry window applies by inheritance from an undocumented dependency default**. `PRAGMA integrity_check` runs on every file-backed open and refuses to proceed on failure, pointing the operator at `<path>.bak` (`storage.rs:903-913`).

**`SessionState` typestate migration: COMPLETE, contrary to `CLAUDE.md`.** `CLAUDE.md:82-87` states that phase 2 — "retiring the string status and folding the PTY handle into the `Live` variant for full typestate" — is *planned*. It is done:
- `SessionStatus` does not exist anywhere in the tree (grep across `src/` and `tests/` returns nothing).
- The `PtyHandle` is owned by `SessionState::Live` and `SessionState::Detached` (`model.rs:126-150`).
- `CLAUDE.md` also omits the `Retryable` variant that now exists (`model.rs:134-136`).
- The only survivor is the string `status` **column**, which is now **write-only**: written on every upsert (`storage.rs:603-611`, `storage.rs:932-941`) and read by exactly one code path, the fallback used when `state_json` is null (`storage.rs:783`).

The documentation is therefore stale and actively misleading for anyone planning follow-up work.

### 1.3 Git, watch, and diff — `src/git.rs` (2,236), `src/watch/`, `src/diff.rs` (985)

**Git invocations.** 30 production invocations in `git.rs` (lines 49–1166; the test module begins at `git.rs:1283`) plus 2 in `cli.rs`. Compliance with `CLAUDE.md`'s Git Command Safety rules is high — see §5.3 for the full audit and the exceptions.

**`src/watch/` is not a filesystem watcher.** Despite the name, it is a regex-over-PTY-output rule engine (`watch/mod.rs:3-10`): it observes terminal snapshots and fires configured effects such as `SendText`. It performs no filesystem I/O.

**Actual filesystem watching** is three `notify` 7.0 `RecommendedWatcher`s — inotify on Linux, FSEvents on macOS. There is **no `PollWatcher` and no debouncer crate** anywhere:
1. Git refs watcher — `workers.rs:1390`, `RecursiveMode::NonRecursive`, with a hand-rolled 5 s debounce. See D-C1: it watches a path that does not exist in a linked worktree.
2. AMQ inject queue watcher — `amq_inject.rs:706`, `RecursiveMode::Recursive`, plus a polling fallback thread at `poll_interval_ms` (default 5,000 ms, `config.rs:692`) for filesystems where `notify` is lossy (`amq_inject.rs:738-758`).
3. Codex rollout watcher — `resume_recovery.rs:194`, `RecursiveMode::Recursive`, short-lived (only while awaiting a new rollout id).

**No watcher is ever pointed at a source tree**, so `target/` and `node_modules/` exposure is nil and no ignore list is needed.

**Watch cadence.** `tick_watch_engines` runs on the **UI thread** at the main loop's ~10 Hz tick and performs **two unconditional full-grid scans per watched session per tick** (`app/mod.rs:3487-3491`) ✓ — one 30-line snapshot for the rule engine and one `busy_scan_lines` snapshot for the AMQ suppression check. `scan_recent_lines` performs a `BTreeMap` probe per cell (`pty.rs:1008`). `WatchEngine::is_active()` exists precisely to short-circuit this and is `#[allow(dead_code)]`, never called (`watch/engine.rs:319`).

**Diff pipeline.** `dispatch_diff` (`workers.rs:3089`) spawns a thread per diff request. The worker calls `git::file_bytes_at_head` for the base and `fs::read` for the working copy (`diff.rs:50-75`), gates on `content_inspector` for binary detection, runs `similar::TextDiff::from_lines` (Myers, whole-file, **no `.deadline()`**), applies `syntect` highlighting, and returns an `Arc<Vec<Line<'static>>>`. The project deliberately does not shell out to `git diff` for display, per `CLAUDE.md`.

### 1.4 Peer, AMQ, resume, purge

**Peer routing.** `dux peer send` → `sync_amq_agents` → `infer_sender` → `resolve_target` → `choose_transport` (`peer.rs:117-158`). The transport rule (`peer.rs:423-459`): if **either** endpoint is a shared-workspace session, AMQ is forced (and an explicit `--transport claude-peers` is rejected); otherwise `auto` sends Claude targets over Claude Peers and everything else over AMQ.

Claude Peers is a hand-rolled HTTP/1.1 POST over a `TcpStream` to `127.0.0.1:<CLAUDE_PEERS_PORT>`, default **7899** (`peer.rs:27`, `peer.rs:548-590`), with a 750 ms connect timeout and 3 s read/write timeouts. Endpoints: `/send-message` with `{from_id, to_id, text}` and `/list-peers` with `{scope:"machine", cwd:"/", git_root:null}` returning `[{id, cwd}]`. Peer identity is matched by canonicalised worktree path (`peer.rs:592-598`).

**AMQ registry.** `peer.rs:662-673` resolves the root from `AMQ_GLOBAL_ROOT`, then `AM_ROOT`, then a sibling `amq` directory next to the dux config root. All registry mutations hold an exclusive `flock` on `<root>/meta/config.lock` (`peer.rs:717-741`), the same lock the bash wrappers take. `marker_state` (`peer.rs:931-958`) classifies an agent directory into `Free` / `Owner{store_id,session_id,wake_pid}` / `Legacy(symlink or non-JSON text)` / `Foreign`, which is what lets the Rust and bash ownership models coexist.

**Inject drain.** The bash bridge writes a queue file; `notify` plus the poll fallback raise `WorkerEvent::AmqInjectScanRequested`; `App::drain_inject_queue_dir` (`inject_runtime.rs:337-504`) scans, claims each file by renaming it to `.inflight.<name>` with `renameat_with(RenameFlags::NOREPLACE)` (`amq_inject.rs:301-320`), validates it (`amq_inject.rs:668-695` — rejects symlinks, files over `max_message_bytes`, and any control character other than `\n` and `\t`), and pushes it onto a per-receiver `VecDeque`. `App::tick_amq_inject` (`inject_runtime.rs:512-715`) then performs **two-phase delivery**: phase 1 writes the body (bracketed paste for Claude and Codex, `inject_runtime.rs:1217-1226`; `macro_payload_bytes` otherwise), and after `phase_delay_ms` phase 2 writes a bare `\r` and unlinks the in-flight file (`inject_runtime.rs:884-968`). The split exists because harnesses with Ink-based input coalesce a single write containing body-plus-CR into a paste buffer, leaving the text unsubmitted.

Delivery is gated by: startup grace, staleness expiry, a quiet-window check against the last operator keystroke, a post-delivery cooldown, PTY presence, and a busy-marker scan of the last `busy_scan_lines` rows (`inject_runtime.rs:520-689`). Once phase 1 has written bytes, **all preflight gates are bypassed** — the only safe forward progress is to send Enter for that same body (`inject_runtime.rs:580-615`).

Safety budgets: `MAX_INJECT_CLAIMS_PER_SCAN = 32`, `MAX_INJECT_PENDING_TOTAL = 128`, `MAX_INJECT_PENDING_PER_RECEIVER = 32`, `MAX_INJECT_ACTIONS_PER_TICK = 16` (`inject_runtime.rs:53-65`).

**Auto-resume.** `auto_resume_all_sessions` (`app/mod.rs:1873-1970`) collects candidates, skipping shared-workspace sessions unless `[workspace].auto_resume_shared`, interrupted spawns, missing worktrees, and worktrees untouched for more than `[auto_resume].stale_days`. It hands the jobs to a semaphore-plus-stagger scheduler on a worker thread (`auto_resume.rs:90-132`, RAII `Permit` so a panic cannot leak a slot). Critically, **the fork itself is marshalled back to the UI thread** via `WorkerEvent::AutoResumeSpawnOnMain` (`app/mod.rs:1979-2032`) for macOS fork-from-worker-thread safety; the worker blocks on an ack so its throttle slot stays occupied across the provider boot window.

"Reject Claude bridge stubs on resume" is `is_resumable_claude_transcript` (`resume_recovery.rs:774-792`) ✓ — it opens the transcript, reads the first JSONL line, and rejects the file if `type == "bridge-session"`, so a placeholder stub is never offered as a resume target.

"Suppress auto-clear while agent is busy" is `should_suppress_auto_clear` (`app/mod.rs:3512-3555`) combined with `runtime.watch_suppress_until`, set for 10 s after each inject phase (`inject_runtime.rs:84`, `:843-846`, `:925-928`), so the Worker-mode postscript's `[task-done]` sentinel cannot false-fire the auto-clear rule.

**History recovery.** `recover_stranded_histories` (`resume_recovery.rs:398-475`) runs on a named worker thread (`workers.rs:2218-2248`) ✓. It scans `~/.claude/projects/**/*.jsonl` and `~/.codex/sessions/**/rollout-*.jsonl`, parsing **every line of every transcript** into a `serde_json::Value` (`resume_recovery.rs:675-704`), and matches them to sessions by historical worktree identity.

**Purge.** `plan_for_session_with_mode` (`purge.rs:406-483`) builds an ordered plan: worktree (isolated sessions only) → provider history dirs → AMQ inbox → log redaction → sqlite row **last**, so a crash mid-purge leaves a recoverable record. Containment is enforced by `resolve_for_containment` + `validate_delete_target` (`purge.rs:488-548`), which require absolute paths, reject `..` components, resolve symlinks against the deepest existing ancestor, and refuse both the category root itself and anything outside it. Confirmation requires the operator to type the literal phrase `PURGE <branch>` (`purge.rs:971-978`).

---

## 2. Memory findings

### 2.1 Measurement method

Cell and row sizes were obtained by compiling a probe against the same `alacritty_terminal 0.26.0` the project resolves (`Cargo.lock:21-23`) and printing `std::mem::size_of`. Per-pane RSS was measured by spawning real PTYs, filling the scrollback to capacity, and sampling process RSS — not by reading the source and estimating. Syntect cost was measured by timing and sampling around `SyntaxCache` construction and a real highlight run.

### 2.2 Derivation of the 20–51 MiB per-pane figure

```
size_of::<alacritty_terminal::term::cell::Cell>()  = 24 bytes   (measured)
size_of::<alacritty_terminal::grid::row::Row<Cell>>() = 32 bytes (measured, per-row overhead)

Row @  80 cols =  80 x 24 + 32 =  1,952 bytes
Row @ 200 cols = 200 x 24 + 32 =  4,832 bytes

Default scrolling_history = 10,000 rows (config.rs:820), plus ~50 visible rows.

@  80 cols: 10,050 x 1,952 = 19,617,600 B = 18.7 MiB grid   -> 20.4 MiB RSS measured
@ 200 cols: 10,050 x 4,832 = 48,561,600 B = 46.3 MiB grid   -> 50.7 MiB RSS measured
```

The ~9% gap between grid arithmetic and measured RSS is allocator overhead plus the `Term` struct, damage tracking, and the vte parser. **16 live panes at 200 columns with saturated scrollback is ~816 MiB in grids alone.**

### 2.3 Scrollback capacity, configurability, and freeing

- **Capacity:** 10,000 lines by default (`config.rs:820`) ✓, fully configurable via `ui.agent_scrollback_lines`, threaded into `Config { scrolling_history }` at `pty.rs:840-843`. The alacritty grid **is** bounded by this value — it is not unbounded.
- **Exited sessions:** the grid **is** freed. `SessionState::into_exited` (`model.rs:318-326`) drops the `PtyHandle`, which drops the `Arc<Mutex<TerminalState>>`.
- **Detached sessions:** the grid is **retained by design** — `SessionState::Detached` still owns the handle (`model.rs:126-150`) so the scrollback survives reconnection. This is deliberate but is exactly what makes an unbounded pane count expensive.
- **Exception:** a PTY whose reader thread is detached on the Drop timeout (`pty.rs:717-727`) leaks its grid regardless of session state — see D-P1.

### 2.4 The memory table

| # | What retains memory | How much | Bounded? | Fix |
|---|---|---|---|---|
| M1 | PTY grid, 10k scrollback, 50x80 | **20.4 MiB RSS** (measured) | by `agent_scrollback_lines` | default 2,000 -> ~4 MiB/pane |
| M2 | PTY grid, 10k scrollback, 50x200 | **50.7 MiB RSS** (measured) | same | same |
| M3 | 16 panes @200 cols, saturated | **~816 MiB** | only by pane count, which is **unlimited** | set `limits.max_panes` |
| M4 | Scrollback watchdog estimate | `BYTES_PER_CELL = 4` (`workers.rs:1202`) ✓ vs real 24 B/cell + 32 B/row | **~6.1x under-count** on grid arithmetic, ~6.7x against measured RSS | `BYTES_PER_CELL = 24` plus row overhead |
| M5 | ...and the watchdog is off | `enable_scrollback_overflow_autodetach = false` (`config.rs:511`) ✓; `max_panes = 0` = unlimited (`config.rs:487`) ✓; `max_companion_terminals = 0` (`config.rs:495`) ✓ | — | enable; cap panes |
| M6 | ...so the 256 MiB cap (`config.rs:499`) ✓ actually fires at | **~1.5-1.7 GiB real** | — | fix M4, or set the cap to 1/6 of the true budget |
| M7 | `vte` `Parser.osc_raw: Vec<u8>` | **unbounded** — the `is_full()` guards are `#[cfg(not(feature = "std"))]` and alacritty enables `std` | **NO** | cap bytes per unterminated OSC in `TerminalState::process` (`pty.rs`) |
| M8 | `App::raw_input_buf` | **unbounded** — an unterminated OSC returns the whole buffer as remainder (`raw_input.rs:208-231`); demonstrated at 100,004 bytes | **NO** | use `append_capped`, as the sibling `loading_input_buf` already does (`app/input.rs:52-65`, cap 64 at `input.rs:19`) |
| M9 | `PendingIngest.buf` per paused pane | 4 MiB cap (`pty.rs:70`) | yes, but xN panes | also resume ingestion on selection change (see D-P4) |
| M10 | vte synchronised-update buffer | 2 MiB VSZ per pane; ~1.9 MiB RSS once a child uses DECSET 2026 | virtual until touched | none without forking vte |
| M11 | Detached reader thread (Drop timeout) | **leaks the entire 20-51 MiB grid** plus 2 fds | **NO** | close the reader fd before waiting so the thread exits |
| M12 | syntect `SyntaxCache` | **1.5 MiB and 0.8 ms to build; 3.7 MiB after highlighting a 2,236-line file** (measured) | rebuilt **per diff** (`workers.rs:3106`), no debounce, no in-flight guard | `static OnceLock<SyntaxCache>` — a ~3-line change, 1.5 MiB once for the process |
| M13 | `classify_untracked_file` | reads **every untracked file in full, every 2 s** (`git.rs:799`); `content_inspector` only needs 1,024 bytes | **NO** | read a 1 KiB prefix |
| M14 | Diff peak | ~4x file size live simultaneously (`diff.rs:50-75`) | **NO** | size cap before reading |
| M15 | `similar::TextDiff::from_lines` | Myers, whole-file, **no `.deadline()`** | **NO** | set a deadline |
| M16 | SQLite session rows | ~1.2-1.7 KB/session; 200 sessions ~ 400 KB | effectively | **not the problem** |
| M17 | `Vec<AgentSession>` inline cost | each element sized for the largest variant `Live { PtyHandle, .. }` ~150-200 B, including exited rows | — | box the handle inside `SessionState` (~3x shrink) |
| M18 | `mmap_size = 128 MiB` (`storage.rs:893`) | VSZ x every connection: App, create-agent worker, and **every `dux peer` process** | virtual | consider lowering for the CLI path |
| M19 | `dux.log` | daily rotation, 7 files retained (`logger.rs:65-69`) ✓ | **no per-file size cap** | add one |
| M20 | Recovery scan `Vec<Transcript>` | up to `MAX_TRANSCRIPT_FILES = 100,000` entries (`resume_recovery.rs:20`) x ~150 B ~ 15 MB, and it reads **every byte of every transcript** (`resume_recovery.rs:675-704`) | count bounded; I/O unbounded | cap by mtime window before parsing |

### 2.5 Conclusion

**The memory problem is PTY grids, not the database.** Three independent defaults conspire: 10,000-line scrollback, an unlimited pane count, and a watchdog that is both disabled by default and 6x wrong. The immediate configuration mitigation for a memory-constrained multi-agent host, requiring no code change:

```toml
[ui]
agent_scrollback_lines = 2000
[limits]
max_panes = 8
enable_scrollback_overflow_autodetach = true
max_total_scrollback_mb = 42   # ~256 MiB of real memory, compensating for the 6.1x estimator error
```

**CPU is the same class of problem.** `tick_watch_engines` does two full-grid walks per watched session per UI tick at ~10 Hz (`app/mod.rs:3487-3491`) ✓, and `busy_snapshot` is computed even though `should_suppress_auto_clear` returns early for non-Worker sessions (`app/mod.rs:3516`). With a `BTreeMap` probe per cell (`pty.rs:1008`), ten sessions is roughly 400k probes/s and ~2k `String` allocations/s **on the render thread**.

---

## 3. Decomposition seams

`audit03` explicitly **deferred** decomposition ("not a fixable audit03 finding … splitting thousands of lines during a security/data-loss fix phase would create risk without behaviour proof", `docs/audits/audit03/06-architecture-modernity.md`). That deferral is why `app/workers.rs` has since doubled from 2,058 to 4,394 lines and the tree grew 19.4%. The seams below are the natural ownership boundaries found by reading the code, sized so no file exceeds 500 lines.

**Note on line counts.** Several files are inflated by inline `#[cfg(test)]` modules that move with their subject. Production-only sizes are given where the difference is material.

### `src/pty.rs` — 2,161 total, **production only lines 1-1357** (803 lines are tests)

| Module | Est. lines | Contents |
|---|---:|---|
| `pty/mod.rs` | 60 | re-exports, `PerSessionEnv` (`:135-160`) |
| `pty/snapshot.rs` | 50 | snapshot types |
| `pty/client.rs` | 340 | `PtyClient::spawn*` (`:150-240`), `write_bytes`, `resize` (`:546`), `Drop` (`:662-727`) |
| `pty/reader.rs` | 130 | reader thread and `reader_loop` (`:255-340`) |
| `pty/handle.rs` | 70 | `PtyHandle` wrapper |
| `pty/terminal.rs` | 250 | `TerminalState` (`:830-1010`), `scan_recent_lines` (`:1008`) |
| `pty/events.rs` | 80 | pause/resume, `PendingIngest` (`:70`), `sync_pause_state` (`:480-486`) |
| `pty/color.rs` | 130 | `NamedColor` -> ratatui mapping (`:1180-1290`) |
| `pty/env.rs` | 100 | `apply_terminal_env*` (`:1297-1332`) |

### `src/raw_input.rs` — 763 total, **production only lines 1-264**

`raw_input/mod.rs` (20) · `raw_input/mouse.rs` (135) · `raw_input/split.rs` (120, the OSC/CSI splitter at `:208-231`).

### `src/storage.rs` — 1,485

`storage/mod.rs` (130, `SessionStore`, `open_connection` `:886-897`) · `store_id.rs` (120) · `migrate.rs` (290, the table at `:129-154` and runner at `:229-244`) · `legacy_column.rs` (200, migration 5 backfill `:264-350`) · `sessions.rs` (330, `upsert_session` `:589-681`, `load_sessions*` `:770-800`) · `prs.rs` (250) · `codec.rs` (110, `state_json` encode/decode) · `testkit.rs` (210).

### `src/model.rs` — 887

`model/mod.rs` (130) · `session_state.rs` (350, `SessionState` + `transition` `:126-330`) · `session_settings.rs` (250, `to_pty_env` `:635-711`) · `agent_handle.rs` (40, `normalize_agent_handle`) · `agent_session.rs` (130).

### `src/git.rs` — 2,236, **production only lines 1-1282**

`git/mod.rs` (60) · `refs.rs` (130, `symbolic_ref`, `rev_parse`) · `status.rs` (230, `changed_files` `:448-588`, `parse_numstat_record` `:894`) · `worktree.rs` (200, add/remove/list `:599-700`) · `safety.rs` (90, `guard_whole_workspace_removal`, `whole_workspace_target_is_within` — the natural home for a shared `run_git(args, timeout)` helper) · `staging.rs` (120) · `mirror.rs` (130) · `links.rs` (110, `ensure_project_worktrees_link_ignored` `:328-370`) · `naming.rs` (90).

### `src/diff.rs` — 985

`diff/mod.rs` (40) · `syntax.rs` (60, the `SyntaxCache` that should become a `OnceLock`) · `render.rs` (200, `diff_file` `:42-75`) · `wrap.rs` (160).

### `src/watch/engine.rs` — 1,002

`engine/mod.rs` (130) · `effect.rs` (60) · `runtime.rs` (180, `observe` / `rebaseline` / `schedule_fire_at` `:415-452`, and the currently-dead `is_active` `:319`).

### `src/peer.rs` — 2,281, **production only lines 1-1332**

| Module | Est. lines | Contents (source ranges) |
|---|---:|---|
| `peer/mod.rs` | 60 | types (`:27-70`), `append_session_env` (`:85-98`) |
| `peer/cli.rs` | 180 | `run_peer*` (`:71-238`), `parse_send_args` (`:240-309`) |
| `peer/routing.rs` | 260 | `infer_sender` (`:326-387`), `resolve_target` (`:389-421`), `choose_transport` (`:423-459`), `session_aliases` (`:633-660`) |
| `peer/transport_amq.rs` | 40 | `amq_send` (`:479-497`) |
| `peer/transport_claude_peers.rs` | 120 | `claude_peers_*` (`:499-598`) |
| `peer/registry.rs` | 380 | `AmqRegistryLock` (`:713-741`), `marker_state` (`:931-958`), `ensure_owner_marker*` (`:968-1017`), `reconcile_amq_root` (`:794-905`), `write_atomic` (`:1076-1097`) |
| `peer/lifecycle.rs` | 290 | tombstone/free (`:1099-1263`), `terminate_wake_pid` (`:1265-1331`) |

### `src/purge.rs` — 1,276, **production only lines 1-1084**

`purge/types.rs` (`:80-294`) · `plan.rs` (`:300-483`) · `containment.rs` (`:485-567`) · `execute.rs` (`:573-765`) · `log_redact.rs` (`:767-961`) · `confirm.rs` (`:963-996`) · `notify.rs` (`:998-1079`).

### `src/app/inject_runtime.rs` — 1,756, **production only lines 1-1307**

`consts.rs` (`:35-102`) · `receiver.rs` (`sanitise_handle`/`match_receiver`, `:154-202`) · `watcher.rs` (`:225-334`) · `drain.rs` (`:337-504`) · `tick.rs` (`:506-745`) · `deliver.rs` (`:805-968`) · `payload.rs` (`:1174-1306`) · `warn.rs` (`:1070-1172`).

### `src/resume_recovery.rs` — 1,260, **production only lines 1-920**

`roots.rs` (`:24-98`) · `codex_capture.rs` (`:100-357`) · `scan.rs` (`:591-744`) · `match.rs` (`:374-589`) · `copy.rs` (`:746-918`).

### Also over 500 lines and out of this brief's scope

`app/input.rs` (13,266), `app/render.rs` (7,565), `config.rs` (4,952), `app/sessions.rs` (4,713), `app/mod.rs` (4,447), `app/workers.rs` (4,394), `keybindings.rs` (2,862), `cli.rs` (1,991), `app/text_input.rs` (1,804).

---

## 4. Rust <-> bash contract

This section is the porting specification for `dux-amq/`. **`dux-amq-rust/` is an empty directory — the port has not started** ✓.

The bash layer comprises three provider wrappers (`dux-amq/wrappers/{claude,codex,gemini}-amq`), the inject bridge (`dux-amq/scripts/dux-amq-inject-bridge`, 307 lines), the signer and verifier (`amq-send-signed`, `amq-receive-verify`), the project-dir encoder (`encode-claude-project-dir`), the doctor (`dux-amq-doctor`, 850 lines), the installer (`install.sh`, 565 lines), and shell wiring (`config/bashrc-additions.sh`).

### 4.1 Environment variables

| Variable | Written by | Read by | Default | Purpose |
|---|---|---|---|---|
| `STATE_ROOT` | bash `config/bashrc-additions.sh:13-17` | wrappers `claude-amq:20`, `codex-amq:7`, `gemini-amq:7`; Rust `purge.rs:105` | `/data/state` (bash); `paths.root.parent()` (Rust) | persistent state root |
| `DUX_HOME` | bash `bashrc-additions.sh:18` | `is_dux_worktree` in all three wrappers (`claude-amq:76`) | `$STATE_ROOT/dux` | worktree containment check |
| `AMQ_GLOBAL_ROOT` | bash `bashrc-additions.sh:19` | wrappers `claude-amq:203`; Rust `peer.rs:663`, `purge.rs:120`, `app/mod.rs:4142` | `$STATE_ROOT/amq` | AMQ queue root |
| `AM_ROOT` | wrappers export `claude-amq:204`; Rust sets it on the `amq` child `peer.rs:486` | bridge `:98`; Rust `peer.rs:663` (fallback), `app/mod.rs:4143` | equals `AMQ_GLOBAL_ROOT` | AMQ root for descendants |
| `AM_ME` | wrappers export `claude-amq:201`; Rust sets it on the `amq` child `peer.rs:487` | bridge `:130`; verifier `:55`; Rust `peer.rs:361` | derived, see §4.2 | receiver identity |
| `AMQ_BIN` | bash `bashrc-additions.sh:25` | `_amq_shell_setup_guarded` `bashrc:26-65` | `$STATE_ROOT/amq-bin/amq` | pinned binary, sha256-guarded before `eval` |
| `DUX_STORE_ID` | **Rust**, per-PTY `peer.rs:88-89` | wrappers `claude-amq:163-173` | none | ownership marker; **must be set together with the next two or the wrapper exits 1** |
| `DUX_SESSION_ID` | **Rust** `peer.rs:86-87` | wrappers `claude-amq:163-173`; Rust `peer.rs:352` | none | ownership marker |
| `DUX_AMQ_HANDLE` | **Rust** `peer.rs:94-97` | wrappers `claude-amq:168`; Rust `peer.rs:361` | none | **managed** handle, used verbatim and never re-sanitised |
| `DUX_PROVIDER` | **Rust** `peer.rs:90-93` | nothing today | none | informational |
| `DUX_PANE` | **Rust**, always `"1"` `pty.rs:1225` ✓ | wrappers `claude-amq:385`; bridge `:210` | none | "running under dux" marker; makes the bridge use the file queue instead of tmux |
| `DUX_PID` | **Rust**, `process::id()` `pty.rs:1226` ✓ | bridge `:216-220` | none | bridge drops messages when the dux instance is dead |
| `DUX_AMQ_VERIFY` | **Rust**, always `"1"` or `"0"` `model.rs:678-681` | wrappers `claude-amq:385`; bridge `:84` | `"0"` (`config.rs:700`) ✓ | strict HMAC verification mode |
| `DUX_SYSTEM_PROMPT` | **Rust**, only when non-blank `model.rs:697-701` | `claude-amq:312` -> `--append-system-prompt`; `codex-amq:220` and `gemini-amq:208` warn and drop | none | per-session system prompt |
| `CLAUDE_AMQ_YOLO` | **Rust**, `"1"` when `yolo_permissions` `model.rs:644` | `claude-amq:285` -> `--dangerously-skip-permissions` | unset | opt-in |
| `CODEX_AMQ_YOLO` | **Rust** `model.rs:647` | `codex-amq:195` -> `--dangerously-bypass-approvals-and-sandbox` | unset | opt-in |
| `CLAUDE_YOLO` | operator | legacy fallback, `claude-amq:285`, `codex-amq:195` | unset | deprecated |
| `CLAUDE_AMQ_SAFE` | operator | `claude-amq:319` — warns only, no effect | unset | deprecated |
| `CODEX_AMQ_BYPASS_HOOK_TRUST` | operator | `codex-amq:210` -> `--dangerously-bypass-hook-trust` | unset | now opt-in (`audit03` P0-09 fixed) |
| `CLAUDE_PEERS_DISABLE` | operator | `claude-amq:295` | unset | skips `--dangerously-load-development-channels server:claude-peers` |
| `CLAUDE_PEERS_PORT` | operator | Rust `peer.rs:549` | **7899** (`peer.rs:27`) | Claude Peers broker port |
| `DUX_AMQ_INJECT_MODE` | operator | wrappers `claude-amq:349` | `via` when `$STATE_ROOT/dux/.tiocsti-state` exists, else `raw` | wake transport selection |
| `DUX_AMQ_FLOCK` | operator / tests | wrappers `claude-amq:207` | `flock` | flock binary override |
| `DUX_AMQ_DRAIN_LIMIT` | operator | bridge `:230` | `20` | `amq drain --limit` |
| `DUX_AMQ_STARTUP_OWNER_PID` | wrappers `claude-amq:390` | bridge `:91-119` | none | one-shot startup backlog drain, waits for the owner generation |
| `DUX_TMUX_TARGET` | operator | bridge `:264` | empty = current pane | tmux target outside dux |
| `AMQ_SECRET_PATH` | operator | signer `:60`, verifier `:13` | `$HOME/.local/share/dux-amq/amq-secret` | HMAC key location |
| `AMQ_EXPECTED_RECEIVER` | bridge `:149` | verifier `:55` | falls back to `AM_ME` | binds the signed recipient to the wake identity |
| `AMQ_AUTH_SKEW_SECONDS` | operator | verifier `:62` | `60` | future-dating tolerance |
| `AMQ_AUTH_WINDOW_SECONDS` | operator | verifier `:63` | `86400` | staleness window and nonce GC horizon |
| `LOCAL_BIN` | operator | wrappers `claude-amq:343` | `$HOME/.local/bin` | bridge lookup path |
| `XDG_DATA_HOME` | environment | Rust `amq_inject.rs:143` | unset -> `~/.local/share` | inject-queue root resolution |
| `XDG_RUNTIME_DIR` | environment | verifier `:94` | `/tmp` | nonce directory parent |
| `DUX_AMQ_DOCTOR_BIN` | operator / tests | Rust `cli.rs:873` | none | doctor script override |
| `CLAUDE_AMQ_SEED_FROM_PARENT` | operator | `claude-amq:91` | unset (**off**) | rsync the parent worktree's Claude history (~100 MB, cross-worktree leak risk) |
| `AMQ_FAKE_ARGV_FILE`, `AMQ_FAKE_FAIL_RECOVER_OWNER` | bats | `dux-amq/tests/fakes/amq` | none | test-only |

### 4.2 Handle derivation (identical in all three wrappers, `claude-amq:163-195`)

1. If **any** of `DUX_STORE_ID`, `DUX_SESSION_ID`, `DUX_AMQ_HANDLE` is set, **all three must be** — otherwise the wrapper prints an error and exits 1 (`claude-amq:164-167`). Then `ME = $DUX_AMQ_HANDLE`, used **verbatim** (`MANAGED_IDENTITY=1`, no re-sanitisation).
2. Otherwise `ME = $AM_ME` if set.
3. Otherwise `basename($PWD)` — but only when `is_dux_worktree` passes, which canonicalises both `$PWD` and `$DUX_HOME/worktrees` through `realpath --` and compares with a trailing-slash **path-segment** test, not a prefix glob (`claude-amq:73-81`).
4. Otherwise `git -C "$PWD" symbolic-ref --quiet --short HEAD`.
5. Otherwise `<provider>-$$`.

Standalone (unmanaged) handles are then lowercased, every character outside `[a-z0-9_-]` becomes `-`, and leading and trailing dashes are trimmed (`claude-amq:190`). Final validation: non-empty, at most 64 characters, matching `^[a-z0-9_-]+$` (`claude-amq:192`).

**The Rust equivalent is `sanitize::amq_handle` (`sanitize.rs:61-79`), which is byte-for-byte equivalent** ✓ — same character class, same case folding, same dash trimming.

### 4.3 Files and directory layout

| Path | Format | Written by | Read by |
|---|---|---|---|
| `$ROOT/meta/config.json` | JSON `{version:1, created_utc, agents:[handle,...]}` | bash wrappers via `jq` under flock (`claude-amq:257-265`); Rust atomically via tmp+rename (`peer.rs:922-925`) | both |
| `$ROOT/meta/config.lock` | empty lock file | — | `flock -x` (bash `claude-amq:279`); `flock(LockExclusive)` (Rust `peer.rs:722-731`) |
| `$ROOT/agents/<handle>/` | directory, mode **0700** | wrappers `claude-amq:215`; Rust `peer.rs:988-993` | both |
| `$ROOT/agents/<handle>/.dux-amq-source` | **JSON** `{store_id, session_id [, wake_pid]}` when dux-managed; a **symlink to `$PWD`** when standalone (legacy) | wrappers `claude-amq:220-250`; Rust `peer.rs:1004` | Rust `marker_state` (`peer.rs:931-958`) handles all four states: `Free`, `Owner`, `Legacy` (symlink or non-JSON text), `Foreign` |
| `$ROOT/agents/<handle>/.wake.lock` | JSON with `.owner.pid`, `.generation` | AMQ binary | wrappers `claude-amq:381`; bridge `:104` |
| `$ROOT/agents/<handle>/.wake.prepared` | JSON with `.generation` | AMQ binary | bridge `:107` |
| `$ROOT/agents/<h>/{inbox/new,inbox/cur,outbox/pending,outbox/sent,receipts}` | Maildir-like | AMQ binary | Rust `amq_activity.rs:12-34` (`has_pending_mail`, `has_recent_activity`) |
| `$ROOT/binary.sha256` | `<sha256>  <path>` | `install.sh` | `bashrc-additions.sh:56` — fails **closed** if the binary exists but the record does not (`bashrc:36-40`) |
| `$STATE_ROOT/dux/.tiocsti-state` | sentinel, existence only | `install.sh` | wrappers `claude-amq:351` |
| `$HOME/.local/share/dux-amq/amq-secret` | raw key bytes, mode 0600 | `scripts/amq-secret-init.sh` | signer `:65`, verifier `:18` |
| `$HOME/.local/share/dux-amq/inject-queue/` | queue root | bridge `:275` | Rust `amq_inject::resolve_queue_dir` (`:139-149`) — honours `[amq.inject].queue_dir`, then `$XDG_DATA_HOME/dux-amq/inject-queue`, then `~/.local/share/dux-amq/inject-queue` |
| `<queue>/<receiver>/<ts_nanos>-<mktemp_suffix>.msg` | **raw body bytes**, no header | bridge `:294-306` | Rust `scan_queue_dir_limited` (`amq_inject.rs:183`) |
| `<queue>/<receiver>/.inflight.XXXXXX` | bridge staging temp, **no `.msg` suffix** | bridge `:294` | deliberately skipped by both sides (`amq_inject.rs:379-387`) |
| `<queue>/<receiver>/.inflight.<name>.msg` | drainer reservation | Rust `amq_inject::claim` (`:301-320`) | Rust `reclaim_stale_inflight_with_max_age` (`:349`) |
| `<queue>/.unrouted/` | fallback when `AM_ME` is unset or sanitises to empty | bridge `:135` | Rust routes it to the **currently-selected session** (`inject_runtime.rs:552-556`) — see D-A4 |
| `<queue>/<receiver>/.expired/` | quarantine for stale messages | Rust `quarantine_expired` (`amq_inject.rs:557-588`) | operator only |
| `<queue>/<receiver>/.rejected/` | quarantine for invalid messages | Rust `quarantine_rejected` (`amq_inject.rs:632-663`) | operator only |
| `${XDG_RUNTIME_DIR:-/tmp}/dux-amq/seen-nonces.d/<nonce>/` | directory-as-mutex; `mkdir` is the atomic check-and-record | verifier `:96-101`; opportunistic GC at `:104-107` | verifier |
| `~/.claude/projects/<encoded>/<uuid>.jsonl` | Claude transcripts | Claude Code | Rust `resume_recovery.rs:591-652`; purge target `purge.rs:429-455` |
| `~/.codex/sessions/**/rollout-*.jsonl` | Codex rollouts | Codex | Rust `resume_recovery.rs:284-304`, `:654-673` |
| `$STATE_ROOT/{claude,codex,gemini}/projects/<encoded>` | provider chat history | providers | purge targets (`purge.rs:131-135`) |
| `<dux config root>/dux.lock` | pid | Rust `lockfile.rs` | `flock`, single-instance guard |
| `<dux config root>/dux.log[.YYYY-MM-DD]` | JSON Lines | Rust `logger.rs:65-80` | purge redaction (`purge.rs:831-861`), doctor |

### 4.4 Inject-queue file lifecycle

1. **Produce.** The bridge writes to `mktemp "$QUEUE_DIR/.inflight.XXXXXX"`, then `mv -f` to `<ts_nanos>-<suffix>.msg` (bridge `:294-306`). The timestamp prefix preserves arrival order; the mktemp suffix guarantees uniqueness across backward clock jumps. Body bytes are stripped of control characters except TAB and LF (bridge `:252`).
2. **Discover.** `scan_queue_dir_limited` (`amq_inject.rs:183-273`) walks one level per receiver directory, skips anything starting with `.` or `.inflight.`, requires the `.msg` suffix, sorts each receiver's files, then **round-robins across receivers** so a deep first directory cannot consume the global cap (this fixed `audit03` P1-17).
3. **Validate the receiver name.** `is_valid_receiver` (`amq_inject.rs:154-169`) accepts `.unrouted` or `[a-z0-9_-]+` not beginning with `-`; anything else is reported as `BadReceiver` and skipped.
4. **Claim.** `renameat_with(..., RenameFlags::NOREPLACE)` to `.inflight.<name>` (`amq_inject.rs:301-320`). `NOREPLACE` is what prevents a recovery from clobbering a newer message (this fixed `audit03` P0-02).
5. **Read and validate.** `read_validated` (`amq_inject.rs:668-695`): reject symlinks, reject files over `max_message_bytes` (default 65,536, `config.rs:696`), reject any body containing a control character other than `\n` or `\t`.
6. **Deliver.** Two phases (§1.4). On success the in-flight file is unlinked (`inject_runtime.rs:896`).
7. **Failure paths.** Post-claim validation failure -> `.rejected/`; staleness beyond `max_message_age_secs` (default 0 = disabled, `config.rs:688`) -> `.expired/`; PTY write failure -> the entry stays pending and phase 1 is retried; phase-2 write failure -> the entry stays in `SubmitPending` and **only Enter is retried**, never the body, so the prompt is not duplicated (`inject_runtime.rs:938-955`).
8. **Restart recovery.** `reclaim_stale_inflight_with_max_age` (`amq_inject.rs:349-462`) renames drainer-format `.inflight.<name>.msg` back to `<name>.msg`, quarantines on `EEXIST`, and **deliberately skips bridge-format `.inflight.XXXXXX` temps** (no `.msg` suffix) so a half-written file is never yanked from a live bridge process.

### 4.5 Locks

| Lock | Mechanism | Scope | Protects |
|---|---|---|---|
| `$ROOT/meta/config.lock` | `flock -x` (bash `claude-amq:279`) and `flock(LockExclusive)` (Rust `peer.rs:722-731`, released on `Drop` `:736-741`) | cross-process, both languages | the agent registry: `meta/config.json` and every `agents/<h>/.dux-amq-source` mutation |
| `<dux root>/dux.lock` | `flock`, held for the App's lifetime (`lockfile.rs`; `app/state/runtime.rs:54-58`) | one dux instance per config dir | released automatically on crash; **`dux peer` does not take it** — see D-C2 |
| `${XDG_RUNTIME_DIR:-/tmp}/dux-amq/seen-nonces.d/<nonce>/` | `mkdir` as an atomic check-and-record (verifier `:98`) | cross-process | DUX2 replay rejection |
| Codex capture coordinator | in-process `Mutex<HashMap<PathBuf, CaptureState>> + Condvar` (`resume_recovery.rs:100-131`) | within one dux process | one Codex rollout capture per worktree — see D-C5 |
| `amq wake` owner claim | `agents/<h>/.wake.lock` JSON, recovered via `amq wake recover-owner --strict` (`claude-amq:381-383`) | cross-process | wake daemon ownership after an unclean exit |

### 4.6 DUX2 envelope

Produced by `dux-amq/scripts/amq-send-signed`, consumed by `amq-receive-verify`. Single line, TAB-separated:

```
DUX2 \t <sender> \t <recipient> \t <utc-iso8601> \t <nonce-hex-24> \t <body-base64> \t <mac-base64>
```

- MAC = `base64(HMAC-SHA256(secret, "DUX2|<me>|<to>|<ts>|<nonce>|<body_b64>"))` — note the canonical form uses `|`, not TAB (signer `:80-83`).
- The body is base64 so tabs, newlines, and trailing bytes survive transport as a single argv element (signer `:75`).
- Nonce is 12 random bytes as 24 hex characters (signer `:73`).
- `--print-only` emits the envelope on stdout and skips `amq send` (signer `:88-91`); the bats tests use this.

**Verifier rejections, all with exit 0** (the AMQ `--inject-via` contract treats non-zero as "retry later", which must never happen for a poison message): unknown magic (`:38`), missing or **extra** fields (`:43`), malformed field encoding (`:48`), recipient != `AMQ_EXPECTED_RECEIVER` (`:56`), future-dated beyond skew (`:74`), older than the window (`:78`), MAC mismatch (`:87`), replayed nonce (`:98`). Setup failures — unreadable or empty secret — exit 1 (`:16`, `:21`).

**Bridge modes.** Skip mode (default): decodes a DUX2 body from base64 without checking the MAC, retains DUX1 compatibility for already-queued messages, and passes anything else through raw (bridge `:155-185`). Strict mode (`DUX_AMQ_VERIFY=1`): calls `amq-receive-verify --emit-base64` with stdin closed and drops silently on empty output (bridge `:138-154`).

### 4.7 Subprocesses Rust spawns toward the bash/AMQ layer

| Call site | argv | Verdict |
|---|---|---|
| `peer.rs:480-489` | `amq send --to <target> --body <msg>`, env `AM_ROOT`, `AM_ME` | **correct** ✓ — matches the installed CLI's documented form `amq send --me <agent> --to <recipients> [--body ...]` |
| `purge.rs:1013-1015` | `amq send <branch> --label purge <body>`, **no env** | **BROKEN** ✓ — see D-A1 |
| `cli.rs:879` | `which dux-amq-doctor` | fine |
| `cli.rs:904`, `cli.rs:933` | `dux-amq-doctor [--json] [--anonymize]`; JSON output is merged with a Rust-produced section, and a non-zero exit falls back to Rust-only output (`cli.rs:910-928`) | fine |

Doctor resolution order (`cli.rs:872-896`): `$DUX_AMQ_DOCTOR_BIN`, then `which dux-amq-doctor`, then `<exe-dir>/../dux-amq/scripts/dux-amq-doctor`.

### 4.8 Signals and TTY mechanics

The wrappers and bridge send **no signals**. Rust sends `SIGTERM` then, after 750 ms, `SIGKILL` to a recorded `wake_pid` — but only after `sysinfo` confirms the process argv still matches `amq ... wake --me <handle> --root <root>` (`peer.rs:1265-1320`), so a recycled pid is never killed. PTY teardown sends `SIGHUP` then `SIGKILL` to the child's **process group** (`pty.rs:662-727`). There are **no FIFOs or named pipes**. `TIOCSTI` is used only by `amq wake --inject-mode raw`, never by dux itself; the bridge exists precisely because `CONFIG_LEGACY_TIOCSTI=n` kernels (Linux 6.2+, Ubuntu 24.04, Debian 12+) silently drop those injections.

### 4.9 Project-directory encoder

Bash `encode-claude-project-dir` and Rust `purge_encoding::encode_str` (`purge_encoding.rs:61-89`) are **logically equivalent** ✓: require an absolute path; strip one trailing `/` unless the path is exactly `/`; map every character outside `[A-Za-z0-9-]` to `-`; runs are **not** collapsed (`__` becomes `--`); case is preserved.

One caveat: the bash version iterates with `${in:i:1}`, which is character-based under a UTF-8 locale but **byte-based under `LC_ALL=C`**. A path containing non-ASCII characters would then produce one `-` per byte instead of one per character. Neither test suite covers this.

---

## 5. Defects

Severity: **High** = data loss, silent incorrectness on a normal path, or resource exhaustion. **Med** = degraded behaviour, latent trap, or a documented contract that is false. **Low** = latent, currently unreachable.

### 5.1 Rust <-> bash contract

**D-A1 — `notify_amq_peers_of_purge` has never worked. Severity: Med.**
`purge.rs:1013-1015` runs `amq send <branch> --label purge <body>`. Verified by execution ✓: the installed CLI answers *"send does not accept positional arguments (got …); use --body to pass message text"*, and the flag is `--labels` (plural), not `--label`. It also sets neither `AM_ROOT` nor `AM_ME`. **Failure scenario:** every hard purge fails to notify peers that the agent is gone; peers simply stop receiving acks with no explanation. It is non-fatal and logged at `warn` (`purge.rs:1024-1031`), which is why it went unnoticed. The correct form is the one already used at `peer.rs:480`.

**D-A2 — enabling strict verification silently disables all peer messaging. Severity: Med (latent).**
Nothing in `src/` ever produces a DUX2 envelope or invokes `amq-send-signed` ✓ (grep across all of `src/` finds only doc-comment mentions in `config.rs:599-612`). `peer.rs:479-497` sends a **raw** body via plain `amq send`. **Failure scenario:** an operator sets `[amq.inject].verify_envelope = true` — a documented, supported knob — so dux exports `DUX_AMQ_VERIFY=1`; the bridge then calls `amq-receive-verify`, which drops unknown-magic input with exit 0 and empty stdout (verifier `:38-41`), and the bridge exits 0 silently (bridge `:151-153`). Every `dux peer send` message vanishes with no error on either side. Latent only because the default is `false` (`config.rs:700`) ✓.

**D-A3 — two implementations of the "single source of truth" encoder. Severity: Med.**
`purge_encoding::encode_str` (`purge_encoding.rs:61-89`) validates that the path is absolute and errors otherwise; `resume_recovery::encode_claude_project_dir` (`resume_recovery.rs:746-761`) does no validation and silently encodes a relative path. The module doc at `purge_encoding.rs:3-8` claims to be the single source of truth. **Failure scenario:** the two drift, and purge deletes a differently-named directory than the one resume reads, orphaning provider history.

**D-A4 — `.unrouted` messages are delivered by UI focus. Severity: Med.**
`inject_runtime.rs:552-556`: a message whose receiver cannot be resolved is delivered to **whatever session the operator currently has selected**. **Failure scenario:** a message intended for agent A is typed into agent B's prompt because the operator happened to be looking at B.

**D-A5 — the AMQ contract is pinned to prose, not a version. Severity: Low.**
The installed binary is `amq v0.61.0` with v0.74.0 available ✓, while the wrapper comments still describe v0.34.0 semantics (bridge `:7-13`). Nothing checks the version at runtime.

### 5.2 Memory and resources

**D-P1 — a detached reader thread leaks the entire grid. Severity: High.**
`pty.rs:717-727` abandons the reader thread after a 1.25 s shutdown budget. The thread still holds `Arc<Mutex<TerminalState>>`, so **20-51 MiB is never freed**, along with 2 file descriptors. It is worse than a timeout race: when `try_wait()` has already reaped the child, `process_group` is `None` *and* `child_exited` is true, so the escalation at `pty.rs:698-703` sends **neither** the group `SIGKILL` nor `child.kill()`. **Failure scenario:** an agent spawns a background grandchild that holds the PTY slave (`sleep 999 &` is enough); the session is deleted; the reader never sees EOF; the grid is pinned for the life of the process. Repeat per session and the machine exhausts memory. Invisible to CI — see §6.

**D-P2 — the scrollback watchdog is 6x wrong, disabled, and unbounded. Severity: High.**
`workers.rs:1202` ✓ uses `BYTES_PER_CELL = 4` against a real 24-byte cell plus 32-byte row overhead. `enable_scrollback_overflow_autodetach` defaults to `false` (`config.rs:511`) ✓, `max_panes` defaults to `0` = unlimited (`config.rs:487`) ✓, `max_companion_terminals` likewise (`config.rs:495`) ✓. **Failure scenario:** the 256 MiB guard (`config.rs:499`) fires at ~1.5-1.7 GiB of real usage, and only if the operator has explicitly enabled it. Found independently by two sub-agents.

**D-P3 — two unbounded buffers reachable from untrusted input. Severity: High.**
`vte`'s `Parser.osc_raw: Vec<u8>` grows without limit because its `is_full()` guards are `#[cfg(not(feature = "std"))]` and alacritty enables `std`; the input is **agent PTY output**, which the threat model treats as untrusted (T13). `App::raw_input_buf` grows without limit because an unterminated OSC returns the whole buffer as remainder (`raw_input.rs:208-231`); demonstrated at 100,004 bytes. The sibling `loading_input_buf` already has the right fix — `append_capped` (`app/input.rs:52-65`) with a cap of 64 (`input.rs:19`).

**D-P4 — deselected panes stay ingestion-paused indefinitely. Severity: Med.**
`sync_pause_state` is only invoked from scroll actions on the *selected* surface (`pty.rs:480-486`). **Failure scenario:** scroll back in pane A, navigate to pane B; pane A silently buffers up to its 4 MiB `PendingIngest` cap (`pty.rs:70`) and displays nothing until the operator returns to it **and** scrolls to the bottom.

**D-P5 — background PTYs are never resized. Severity: Med.**
`render.rs:1197-1207` compares against a single global `last_pty_size` and assigns it *before* resizing only the selected surface, so re-selecting a pane cannot repair a stale winsize. **Failure scenario:** resize the terminal while pane B is hidden; B's child keeps the old winsize forever and renders wrapped garbage.

**D-P6 — `foreground_process_name` forks on the UI thread. Severity: Med.**
`workers.rs:1041-1046`, once per companion terminal every ~2 s. On macOS this is a `fork` + `exec` of `ps` on the render thread. Directly violates `CLAUDE.md`'s "all periodic or potentially-blocking work must run in background workers" rule.

**D-P7 — syntect cache rebuilt per diff. Severity: Med.**
`workers.rs:3106` constructs a `SyntaxCache` for every diff request — 1.5 MiB and 0.8 ms each, with no debounce and no in-flight guard, so holding an arrow key queues one rebuild per keystroke. Fix is a `static OnceLock<SyntaxCache>`; `SyntaxSet` and `ThemeSet` are `Sync`.

**D-P8 — untracked files are read in full every 2 s. Severity: Med.**
`git.rs:799` `fs::read`s each untracked file entirely for `content_inspector`, which only inspects 1,024 bytes. **Failure scenario:** a worktree with a large untracked artifact re-reads it every polling interval.

**D-P9 — no deadline on `similar`, no size cap on diff input. Severity: Med.**
`diff.rs:50-75` reads both sides fully (~4x file size live) and `TextDiff::from_lines` runs Myers with no `.deadline()`. **Failure scenario:** opening a large generated file wedges a diff worker indefinitely.

**D-P10 — `dux.log` has no per-file size cap. Severity: Low.**
`logger.rs:65-69` ✓ rotates daily and keeps 7 files, but a single day's file is unbounded.

### 5.3 Git command safety audit

Audited every git invocation against `CLAUDE.md`'s Git Command Safety rules: 30 production invocations in `git.rs` (lines 49-1166; tests begin at `git.rs:1283`) and 2 in `cli.rs`.

**Compliant — the rubric is largely honoured:**

| Rule | Result |
|---|---|
| `git status --porcelain=v1 -z`, never `--short` | **4 of 4 compliant** |
| `--numstat -z` for diff stats | **2 of 2 compliant** |
| `symbolic-ref --quiet --short HEAD` over `branch --show-current` | compliant; **zero** occurrences of `--show-current` in the tree |
| `-c color.diff=false` where no plumbing alternative exists | applied at `git.rs:967` |
| `core.quotePath` hazard | neutralised by `-z` throughout |
| Plumbing over porcelain | `cat-file`, `rev-parse`, `for-each-ref` used consistently |

**Violations:**

**D-G1 — `dux reset` corrupts the git worktree registry. Severity: Med.** ✓ verified firsthand.
`cli.rs:706-715` runs `git worktree remove --force <path>` with **no `-C`, no `current_dir`, no `--` terminator, and the exit status never checked** — only a spawn failure produces an error (`cli.rs:710-715`). It then calls `fs::remove_dir_all` regardless (`cli.rs:716-723`) with no `git worktree prune`. **Important nuance:** two containment guards run first — `whole_workspace_target_is_within` (`cli.rs:695-702`) and `guard_whole_workspace_removal` (`cli.rs:703`) — so this **cannot** delete outside the managed worktrees root. The defect is a stale git registration plus a swallowed error, not arbitrary deletion.

**D-G2 — missing `--` on destructive commands. Severity: Med.**
`git.rs:655` (`worktree remove`), `git.rs:681` and `cli.rs:727` (`branch -D`; the latter discards the `Result` entirely with `let _ =`). A branch or path beginning with `-` is parsed as a flag. `rename_branch` (`git.rs:1088`) shows the correct pattern.

**D-G3 — non-UTF-8 path handling is inconsistent. Severity: Med.**
`git.rs:861`, `git.rs:898`, and `git.rs:372` decode paths with `from_utf8_lossy`; only `parse_registered_worktrees` (`git.rs:599`) uses `OsString::from_vec` correctly. **Failure scenario:** a file whose name contains non-UTF-8 bytes round-trips as U+FFFD, so stage, discard, and diff then operate on a path that does not exist; `parse_numstat_record` (`git.rs:894`) drops the record entirely and the UI shows `+0 −0`. The only coverage, `tests/git_portability.rs`, is Linux-gated and covers *directory* names only.

**D-G4 — no timeout on any git subprocess. Severity: High.**
Every invocation uses `.output()` with no deadline. **Failure scenario:** a stuck `index.lock` (a crashed `git gc`, a network filesystem stall) permanently wedges `commit_in_flight`, `pending_deletions`, `pulls_in_flight`, and the changed-files poller **with no user-visible error** — precisely the silent waiting `CLAUDE.md`'s "Prefer explicit failure over silent waiting" tenet forbids.

**D-G5 — commit-message generation can execute repo-controlled code. Severity: Med.**
`git.rs:967` runs `git -c color.diff=false diff --cached` without `--no-ext-diff`. **Failure scenario:** a malicious repository sets `diff.external` or a `textconv` filter in its local config; git executes that program, and its output is then fed to an LLM for commit-message generation. Malicious repositories are explicitly in scope per `SECURITY.md` T1.

**D-G6 — two git calls on the UI thread. Severity: Med.**
`git::branch_exists` inside the create-agent key handler (`app/input.rs:3279`) and `ensure_project_worktrees_link` at bootstrap (`app/sessions.rs:245`, which includes an `fsync`).

### 5.4 Correctness

**D-C1 — the refs watcher watches nothing for agent sessions. Severity: High.** ✓ verified firsthand.
`workers.rs:1449-1453` builds `<worktree_path>/.git/refs/heads` and gates on `is_dir()`. In a `git worktree add` linked worktree — which is every dux agent session — **`.git` is a regular file** containing `gitdir: …`, so the path is never a directory and `watch()` is never called. The map is also populated only once, from `GhStatusChecked` (`workers.rs:238`), only when GitHub integration is enabled, with no add-on-session-create and no unwatch-on-delete. **Failure scenario:** PR/branch change detection has been silently running on the 45 s poll fallback rather than the event path.

**D-C2 — `dux peer` writes SQLite without the single-instance lock. Severity: High.**
`main.rs:106-108` dispatches `dux peer` without acquiring `dux.lock`, while the TUI holds it. `sync_amq_agents` runs migrations and can `UPDATE agent_sessions` (`peer.rs:111`, `peer.rs:842`). Since `CLAUDE.md` instructs every agent to route through `dux peer send`, this is the hot path, not an edge case. The 5 s `busy_timeout` inherited from rusqlite (§1.2) means contention manifests as a stall then an error rather than instant failure, but the **logical** race — two processes doing read-modify-write on the same rows — is unmitigated.

**D-C3 — no forward-compatibility guard on `user_version`. Severity: High.**
`storage.rs:166-178` applies migrations for versions above the current one but does not refuse a database whose `user_version` **exceeds** the binary's known maximum. **Failure scenario:** the operator runs an older dux against a newer database; it opens successfully and writes rows in the old shape, corrupting state the newer binary expects.

**D-C4 — lost updates on cross-statement read-modify-write. Severity: Med.**
`write_provider_session_id` (`storage.rs:704-730`) and `upsert_session` (`storage.rs:589-681`) read then write without `BEGIN IMMEDIATE`. The in-process `Arc<Mutex<Connection>>` does not span processes, so D-C2's second connection defeats it.

**D-C5 — a Codex workspace can be permanently blocked. Severity: Med.**
`impl Drop for CodexCapture` (`resume_recovery.rs:337-350`) inserts `CaptureState::Blocked` when dropped while still active, and **nothing ever clears that state** — `release_capture` (`:352-357`) is only reached via `resolve()` or `abort()`. `begin_with` bails on `Blocked` (`:160-162`). **Failure scenario:** a panic or an early `?` return drops the capture; every subsequent Codex launch in that worktree fails with "Codex capture is blocked for this workspace" until dux is restarted, because the coordinator is a process-lifetime `OnceLock` (`:128-131`).

**D-C6 — no mutex-poison recovery in the PTY path. Severity: Med.**
All 23 `lock()` sites in `pty.rs` correctly avoid `unwrap`, but on poison `reader_loop` (`pty.rs:336`) silently discards every byte and **never sets `exited`**, so the exit poller never reaps the session. **Failure scenario:** a panic anywhere holding the terminal mutex leaves a frozen pane with no error message and its grid still resident.

**D-C7 — `sanitize::truncate(s, 0)` underflows. Severity: Low.**
`sanitize.rs:52` computes `max_chars - 1` on a `usize`. The only call site passes 300 (`peer.rs:586`), so it is currently unreachable.

**D-C8 — fragile `expect` in the inject tick. Severity: Low.**
`inject_runtime.rs:603`, `:698`, `:699` use `.expect("head exists per phase peek")` after re-borrowing across `&mut self` calls. The invariant holds today because `maybe_warn_timeout` and `log_holding` do not pop the queue, but a future helper that does would panic the whole TUI.

**D-PU1 — the purge dry-run contract is false on both clauses. Severity: Med.**
`purge.rs:47-51` claims "Every step honors `dry_run`" and "The integration tests assert this explicitly per category." Both are false.
- *Clause A:* `PurgeItem::SharedProviderHistory` (`purge.rs:675-688`) **never reads `dry_run`**; it returns `Skipped` or `Error` unconditionally. **Failure scenario:** a dry run of a shared-workspace plan returns `PurgeOutcome::Error` and `had_errors() == true`, so the rehearsal of an irreversible operation reports failure. All five other arms honour the flag (`purge.rs:735-737`, `:702-704`, `:811`, `:754-756`).
- *Clause B:* there is exactly **one** `execute(..., true)` call in the entire suite (`tests/purge_integration.rs:366`), and its coverage of 5 of 6 variants is *incidental* — a single blanket `all(matches DryRun)` assertion that happens to reach them because the fixture session is non-shared.

### 5.5 Incomplete migrations

**`SessionState` typestate: COMPLETE — the documentation is what is stale.**
See §1.2. `CLAUDE.md:82-87` describes `SessionStatus` (which no longer exists), calls phase 2 "planned" (it is done), and omits the `Retryable` variant (`model.rs:134-136`). **Remaining work is a documentation fix plus, optionally, a migration 0007 to drop the write-only `status` column** (`storage.rs:603-611`, `:932-941`; sole reader `storage.rs:783`).

**`tracing` migration: ~69% complete.** ✓ counted directly:

| Metric | Count |
|---|---:|
| `tracing::{info,warn,error,debug}!` call sites | **113** |
| Live `crate::logger::{info,warn,error,debug}` shim calls | **50** |
| ...of which in `app/workers.rs` | 32 |
| ...in `app/sessions.rs` | 11 |
| ...in `app/mod.rs` | 4 |
| ...in `theme.rs`, `storage.rs`, `pty.rs` | 1 each |
| `tracing!` calls setting an explicit `target:` | **all of them** — the only apparent misses are doc comments |

`logger.rs` is a genuine thin shim over `tracing` (`logger.rs:115-148`), routing through target `dux::legacy` after `sanitize::for_terminal`.

**D-M1 — the incomplete `tracing` migration breaks GDPR purge. Severity: High.** ✓ **proven.**
`purge.rs:942-961` redacts a JSON Lines record only when `obj["fields"]["session_id"]` equals the target. `tests/logger_jsonlines.rs:86-92` asserts that legacy-shim records carry `fields.message` and nothing else — they have **no `fields.session_id`**. Therefore **no legacy record is ever redacted**. And legacy call sites interpolate identifiers directly into the message text, for example `app/sessions.rs:1063-1066`, which logs `"deleting session {} at {} (delete_worktree=true, async)"` with `session.id` and `session.worktree_path`, and `app/mod.rs:2027-2029`, which logs `"auto_resume_on_start: failed to spawn session {session_id}: {e}"`. **Failure scenario:** after `dux session purge --hard`, session UUIDs and full worktree paths (which encode branch names, often customer- or feature-identifying) remain in `dux.log` and its 7 rotated siblings. This is the concrete, GDPR-relevant reason to finish the migration.

### 5.6 Thread, join, and poison issues

| Issue | Location | Severity |
|---|---|---|
| Reader thread detached on timeout, grid leaked (D-P1) | `pty.rs:717-727` | High |
| Escalation skipped when the child is already reaped (D-P1) | `pty.rs:698-703` | High |
| Poisoned terminal mutex discards bytes and never sets `exited` (D-C6) | `pty.rs:336` | Med |
| `CodexCapture::Drop` blocks a workspace permanently (D-C5) | `resume_recovery.rs:337-350` | Med |
| `auto_resume` `Permit` correctly releases on panic — **no defect**, RAII is right | `auto_resume.rs:48-76` | — |
| `run_scheduler` joins every worker before returning — **no defect** | `auto_resume.rs:129-131` | — |
| Only 7 `lock().unwrap()/expect()` in the whole tree, all in `resume_recovery.rs` (4), `auto_resume.rs` (2), `storage.rs` (1) | — | Low |
| ~34 total `unwrap()/expect()` in non-test code across all of `src/` | — | Low (the codebase is disciplined here) |

---

## 6. Coverage gaps

`cargo test` exits 0 with **1,139 distinct tests on macOS** (1,056 unit + 83 integration; the frequently quoted 1,149 double-counts the lib suite, which reruns under the `main.rs` bin target). The gaps below are ordered by consequence.

### G1 — The two-phase inject state machine has **zero** executable coverage. Consequence: High.

`tick_amq_inject` (`inject_runtime.rs:512`) has exactly one caller — the run loop (`app/mod.rs:1697`); `drain_inject_queue_dir` (`:337`) has exactly one — `workers.rs:790`. **No test invokes either**, so `deliver_inject_body` (`:805`) and `deliver_inject_enter` (`:884`), both private, are unreachable from tests. The unit tests at `inject_runtime.rs:1308+` never construct an `App`; they import only free functions and the `AmqDeliveryPhase` enum (`:1310-1314`), covering byte encoding, `sanitise_handle`, `match_receiver`, the postscript, and three predicates.

Uncovered: the `TypeBody -> SubmitPending` transition; the `body_typed` / `body_typed_at` mutation (`:696-702`); the phase-2 `\r` write plus `fs::remove_file` (`:896`); `pop_front` on success (`:606`); the `MAX_INJECT_ACTIONS_PER_TICK` budget break (`:545`, `:657`); the busy-marker hold (`:673`); the startup-grace return (`:522`); and the retry-Enter-without-retyping branch (`:936-952`).

The two integration tests whose names promise this coverage do not provide it: `tests/amq_inject_integration.rs:40` and `:158` exercise `scan_queue_dir` / `claim` / `read_validated`, and then **the test itself** writes the body and `\r` to a `cat` PTY and **the test itself** calls `fs::remove_file` (`:94`). Three phase-delay unit tests (`:1649`, `:1662`, `:1677`) re-derive the comparison in the test body instead of calling the gate at `:585-588`; `:1677` asserts `x >= Duration::ZERO`, which is unconditionally true.

**Test that would close it:** an `App`-level test that writes a real queue file, runs `drain_inject_queue_dir`, then ticks `tick_amq_inject` twice across the phase delay, asserting the PTY received body-then-`\r` as two separate writes and that the in-flight file is unlinked. Fixtures already exist (`app/sessions.rs:2847 test_app_with_sessions`, `app/input.rs:6238 test_app`), so this is buildable today. This matters because the recent bug-fix commits ("suppress auto-clear while agent is busy", "reject Claude bridge stubs on resume") landed in exactly this code.

### G2 — The PTY grid leak is invisible to CI. Consequence: High.

The sole Drop test, `dropping_pty_client_with_background_descendant_returns_promptly` (`pty.rs:2075-2103`), spawns a `sleep 30` descendant with `trap '' HUP`, drops on a worker thread, and asserts only that `rx.recv_timeout(2s)` succeeds. It is **specifically designed to tolerate** the detach branch at `pty.rs:721-727`, so it passes whether or not the reader thread and its terminal buffers are reclaimed. Nothing asserts that `handle.join()` ran, that the reader thread exited, or that the descendant's process group was `SIGKILL`ed (`:698-700`).

**Test that would close it:** assert the reader thread actually terminated (a `JoinHandle` result or an `Arc::strong_count` on the terminal dropping to zero) and that the descendant received the signal — i.e. assert reclamation, not promptness.

### G3 — The Rust <-> bash seam is untested on both sides. Consequence: High (this is the interface being ported).

Each half is covered separately:
- **Rust, config to env:** `config.rs:3533` pins the `verify_envelope` default; `config.rs:3555` pins the TOML round-trip; `tests/pty_integration.rs:270` covers `to_pty_env` override precedence. `tests/pty_integration.rs:210` is the **only** test that gets `DUX_AMQ_VERIFY=1` into a real PTY child — and that child is `/bin/sh -c printf`, not a wrapper.
- **Bash, env to behaviour:** `dux-amq/tests/inject-bridge.bats` sets `DUX_AMQ_VERIFY` **by hand** in three tests (`:157` strict drops unsigned, `:171` strict drops MAC mismatch, `:335` strict signed delivery), with skip-mode counterparts at `:420`, `:432`, `:450`. `dux-amq/tests/amq-auth.bats` covers the verifier in ~12 tests including replay (`:79`), MAC mismatch (`:96`), wrong recipient (`:180`), and concurrent replay (`:198`).

**Neither side crosses the seam.** No Rust test spawns `dux-amq-inject-bridge` or `amq-receive-verify`; no bats test derives `DUX_AMQ_VERIFY` from dux. The halves meet only at `claude-amq:385` / `codex-amq:230` (`[[ -n "${DUX_PANE:-}" && "${DUX_AMQ_VERIFY:-0}" != "1" ]]`), exercised by neither. They are also **separate CI jobs** — bats runs under `.github/workflows/overlay-ci.yml:38`, not under `cargo test`.

**Test that would close it:** a Rust integration test that spawns the real wrapper against a scratch `$AMQ_GLOBAL_ROOT` with a fake `amq`, and asserts the queue file the bridge produces is exactly what `read_validated` accepts. This is also how D-A2 would have been caught.

### G4 — `tests/git_portability.rs` runs zero tests on macOS. Consequence: Med.

`#![cfg(target_os = "linux")]` at `:16`; `cargo test` reports `running 0 tests` ✓. Both tests (`:31`, `:83`) also `return` early rather than skip-marking when git is absent (`:41`, `:89`), so a git-less CI leg passes green. This is the only coverage for D-G3, and it tests *directory* names only, never file names inside a numstat record.

### G5 — Purge dry-run coverage is incidental, and one category is uncovered. Consequence: Med.

Per §5.4 D-PU1: one `execute(..., true)` call site total (`tests/purge_integration.rs:366`), a single blanket assertion, and `SharedProviderHistory` never reached because the fixture session is non-shared — all four shared-plan tests (`:579`, `:626`, `:654`, `:703`) pass `dry_run = false`.

**Test that would close it:** a dry run over a shared-workspace plan asserting every entry is `DryRun` and `had_errors() == false`.

### G6 — App-level call sites of the quarantine paths are untested. Consequence: Med.

The pure helpers are unit-tested: `is_valid_receiver` (`amq_inject.rs:819`), `quarantine_expired` (`:988-1001`), `quarantine_rejected` (`:971-985`), the reclaim sweeps (`:1069`, `:1093`) and the collision case (`:1141-1165`). But every **App** call site is unreachable from tests because it lives inside `drain_inject_queue_dir` / `tick_amq_inject`: pre-claim expiry (`inject_runtime.rs:400`), post-claim rejection plus `set_warning` (`:476`), startup reclaim reporting (`:259`), and `expire_pending_amq_head_if_stale` (`:1008`) — including the `body_typed` guard at `:988` that must **not** expire a message already typed into a prompt.

The `.unrouted` fallback (`inject_runtime.rs:552-556`) is untested in Rust; only the bridge's side is covered, in bats (`inject-bridge.bats:190`, `:206`).

### G7 — No test exercises two processes or two connections. Consequence: High (D-C2, D-C3).

No test opens two `SessionStore` handles on the same path — all 74 `SessionStore::open` calls use fresh tempdirs. `lockfile.rs:254 second_acquire_same_path_reports_running_with_pid` takes both flocks **in the same process**, proving per-fd exclusivity rather than cross-process mutual exclusion. Nothing spawns a second dux binary (no `CARGO_BIN_EXE` usage anywhere). `open_sets_wal_mode` (`tests/storage_integration.rs:35`) asserts the PRAGMA is set but never provokes contention. There is **no downgrade test** for D-C3.

### G8 — Weak tests worth knowing about. Consequence: Low, but they inflate the apparent count.

Three of nine `tests/pty_integration.rs` tests (`:7`, `:35`, `:123`) exercise **portable-pty and alacritty directly, not dux code** — and the comment at `:8` claiming `PtyClient` cannot be imported is refuted by `:210`, which imports it fine. `pty_resize:444` asserts `pair.master.resize()` and never `PtyClient::resize` (`pty.rs:546`); its doc comment says "doesn't panic". `tests/auto_resume.rs:86` tests `is_stale` in isolation while nothing exercises the gate inside `auto_resume_all_sessions`. `tests/limits.rs` pins defaults and TOML round-trips with **zero enforcement coverage** (its own header at `:17-23` admits the refusal paths live elsewhere). `tests/sanitize.rs` re-exports unit tests and touches no production call site.

### What is genuinely well covered

`tests/purge_integration.rs` (22 tests: symlink escapes in both directions, foreign-inbox protection, per-category retry-after-failure, documented ordering pinned at `:485`, root/parent rejection at `:797`) — the strongest file in the suite, with no weak tests found. `tests/watch_engine_integration.rs` (5 tests driving real PTYs end-to-end, including the `[task-done]` auto-clear rule at `:156`). `tests/storage_migrations.rs` (14 tests, including atomic rollback of migration 5). `src/lockfile.rs` (11 tests including 4 for the read-retry race). `tests/pty_integration.rs:210` — the one test proving the per-session env contract reaches a real PTY child.

---

## 7. Risk register

| Risk | Severity | Defect | Covered today? |
|---|---|---|---|
| OOM on a many-agent host from retained PTY grids | High | D-P2, M1-M6 | `tests/limits.rs` covers cap *config*; **nothing** covers the 6x estimator error |
| Reader-thread detach leaks 20-51 MiB per occurrence | High | D-P1 | invisible to CI (G2) |
| Purge leaves session ids and worktree paths in `dux.log` | High (GDPR) | D-M1 | `tests/logger_jsonlines.rs` proves the record shape but never connects it to redaction |
| Two-phase inject regression ships unnoticed | High | — | **zero executable coverage** (G1) |
| `dux peer` writes SQLite unserialised against the TUI | High | D-C2, D-C4 | **nothing** — no test races two connections (G7) |
| Older binary silently writes a newer database | High | D-C3 | **nothing** — no downgrade test |
| Git subprocess hangs and wedges the UI with no error | High | D-G4 | **nothing** — there is no timeout to test |
| PR/branch detection silently degraded to polling | High | D-C1 | **nothing** |
| Unbounded buffers fed by untrusted agent output | High | D-P3 | **nothing** |
| Non-UTF-8 filenames mis-staged or shown as `+0 −0` | Med | D-G3 | Linux-gated, directories only (G4) |
| Enabling a documented config knob silently drops all peer messages | Med | D-A2 | seam untested on both sides (G3) |
| Purge peer notification has never worked | Med | D-A1 | not covered; the bats fake `amq` accepts any argv and exits 0 |
| Dry run of a shared-workspace purge reports failure | Med | D-PU1 | uncovered category (G5) |
| Codex workspace permanently blocked until restart | Med | D-C5 | not covered |
| Malicious repo executes code via `diff.external` into an LLM prompt | Med | D-G5 | not covered; T1 is in scope per `SECURITY.md` |
| `dux reset` leaves a stale git worktree registration | Med | D-G1 | this site untested; `orphan_worktrees.rs:274-388` is the best-tested destructive path |
| Cross-agent message misdelivery by UI focus | Med | D-A4 | Rust side untested (G6) |
| Hidden panes stall or render with a stale winsize | Med | D-P4, D-P5 | not covered |
| SQLite tombstones and free pages never reclaimed | Low | — | no `VACUUM` anywhere in the tree |

**Data-safety posture, in fairness.** The destructive paths are guarded more carefully than most of this list implies: purge enforces absolute-path, no-`..`, symlink-resolving containment with an explicit refusal of category roots (`purge.rs:488-548`), requires the operator to type `PURGE <branch>` (`purge.rs:971-978`), orders the sqlite row deletion **last** so a crash is recoverable (`purge.rs:38-45`), and verifies exact `{store_id, session_id}` ownership before touching an AMQ inbox (`purge.rs:714-732`). `dux reset` runs two independent containment guards before deleting anything (`cli.rs:695-703`). Worktrees are never removed without confirmation. The gaps above are real, but they sit on top of a conservative foundation.

---

## 8. Open questions

1. Was `BYTES_PER_CELL = 4` (`workers.rs:1202`) a deliberate "detach early" heuristic or a measurement error? It errs in the **wrong** direction — a low estimate detaches *late*, not early.
2. Has the refs watcher (D-C1) ever fired for a linked worktree, or has PR detection been running on the 45 s poll fallback since it was written?
3. Is the per-diff `SyntaxCache` (D-P7) deliberate beyond "a `&SyntaxCache` is not reachable from a worker thread"? A `static OnceLock` is `Sync` and would be roughly a three-line change.
4. Should `dux peer` take the single-instance lock — serialising every agent message behind the TUI — or should the writes move behind `BEGIN IMMEDIATE` with retry? These have very different UX consequences and the choice should be explicit.
5. Is the write-only `status` column worth its `NOT NULL` slot, or should a migration 0007 drop it now that `SessionState` is authoritative?
6. Is there a stated retention window for soft-deleted session rows? Today they persist indefinitely and are read on **every** `dux peer send` via `load_sessions_including_deleted` (`peer.rs:113`).
7. Should `changed_files` degrade to `--untracked-files=normal` beyond N entries, and skip `classify_untracked_file` above a size threshold (D-P8)?
8. Does the bash encoder's `${in:i:1}` diverge from the Rust encoder under `LC_ALL=C` for non-ASCII paths (§4.9)? Neither suite tests it, and a divergence orphans provider history.
9. Was the `amq` CLI contract (D-A1) ever exercised end to end, or has the fake `amq` in bats — which accepts any argv and exits 0 (`dux-amq/tests/fakes/amq:31`) — masked it since day one?
10. Should the bats suite run in the same CI job as `cargo test`? Today the Rust <-> bash contract, the very interface being ported, is the one interface with tests on both sides and coverage on neither (G3).

---

## Appendix: what `audit03` found that is now fixed

Verified as remediated since baseline `3d52074`, which is useful context for judging the codebase's maintenance posture:

| audit03 finding | Status now |
|---|---|
| P0-02 stale-inflight recovery overwrites a newer message | **Fixed** — `renameat_with(RenameFlags::NOREPLACE)` plus quarantine (`amq_inject.rs:414-448`) |
| P1-17 AMQ scan caps an arbitrary subset before sorting | **Fixed** — per-receiver sort then round-robin (`amq_inject.rs:238-268`) |
| P1-18 busy/missing-PTY deliveries never warn | **Fixed** — `timeout_warning_due` now seeds via `entry().or_insert(now)` (`inject_runtime.rs:1163-1172`) |
| P1-19 `u64::MAX` quiet-window holds forever | **Fixed** — `ALWAYS_DELIVER_QUIET_WINDOW` (`inject_runtime.rs:1265-1290`) |
| P1-15 migrations not atomic | **Fixed** — one transaction spanning DDL and version bump (`storage.rs:229-244`) |
| P1-14 backup worker never started | **Fixed** — wired at `app/mod.rs:1675-1680` |
| P1-27 lifecycle persistence failures swallowed | **Fixed** — no `let _ = ... upsert_session` remains in the tree |
| P1-03 `_unrouted` is both a handle and a sentinel | **Fixed** — the sentinel is now `.unrouted` with a leading dot (`amq_inject.rs:45`) |
| P1-04 strict HMAC has no coherent boundary | **Mostly fixed** — receiver binding (verifier `:55-60`), atomic `mkdir` nonce (`:98`), extra-field rejection (`:44`), base64 body so multiline survives |
| P1-02 wrapper identity claim race | **Fixed** — `flock -x "$ROOT/meta/config.lock"` around `claim_and_register_locked` (`claude-amq:279`) |
| P0-09 Codex hook-trust bypass on by default | **Fixed** — now opt-in via `CODEX_AMQ_BYPASS_HOOK_TRUST` (`codex-amq:210-213`) |
| Very large ownership modules | **Not fixed, and worse** — deferred by audit03; `app/workers.rs` has since doubled (2,058 -> 4,394) and the tree grew 19.4%. §3 is the proposed remedy. |
