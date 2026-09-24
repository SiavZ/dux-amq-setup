# TUI hot reload for dux

Goal: swap the dux binary without killing running agents, the way jcode does.

## How jcode does it

jcode is a client/server split. The agent turn runs in a **separate server
process**; the TUI is only a viewer. So reload is cheap:

1. `save_input_for_reload()` writes input + queued messages to
   `~/.jcode/client-input-<session>` (`state_ui.rs`).
2. `reload_requested = Some(session_id)`, loop quits (`tui_lifecycle_runtime.rs`).
3. `hot_reload()` calls `replace_process()` -> `exec()` onto the new binary with
   `--resume <id>` (`src/cli/hot_exec.rs`).
4. Generation 2 reconnects to the **still-running server** and restores input.

Two guards: `if self.is_processing { return false }` never interrupts a turn,
and `has_newer_binary()` only reloads when the payload mtime actually changed.

## Why dux is different

dux has no server for agents. `PtyClient` owns the child **directly**, and
`shutdown_ptys_interruptible` terminates every provider on exit.

That made a daemon split look necessary. It is not.

## What was verified (probes, not reasoning)

| Question | Method | Result |
|---|---|---|
| Do children survive `exec`? | Rust probe: spawn, then exec | **Yes**, reparented to PPID 1 |
| Is the PTY master CLOEXEC? | Read `portable-pty/src/unix.rs:64`, then `F_GETFD` on a live master | **Yes**, closed on exec |
| Can it survive anyway? | Clear `FD_CLOEXEC`, exec, write+read the inherited fd | **Yes**, full round trip |
| Same child, or a new one? | Compare `$$` and a pre-exec shell var across the exec | **Same pid**, state intact |
| Is it still *our* child? | After exec, `SIGKILL` + `waitpid` from the new image | **Reaped normally** |

The last row matters: `exec` replaces the image, not the process, so the
parent-child relationship survives and ordinary signals and `waitpid` work.

## How to use it

Open the command palette and run **`reload-binary`**.

It refuses, on the status line, when:
- the binary on disk is not newer than the one running
- any agent is mid-turn (a reload clears transcripts)
- a PTY cannot be handed over

Every refusal happens before anything is touched, so a refused reload leaves the
session exactly as it was.

## What is built

- `pty_reattach` — adopt an inherited master as a `MasterPty` (portable-pty's own
  type has private fields and no from-fd constructor, but the trait is small)
- `pty_adopt_child` — wrap the surviving process as a `Child` (thin: same
  `waitpid`/`kill` the original made)
- `reload_handoff` — the manifest naming which fd belongs to which tab
- `reload_policy` — the guards, the exec, and the `--reload-handoff` flag
- `PtyClient::prepare_for_reload` / `adopt_after_reload`
- `Engine::prepare_reload_handoff` / `restore_reload_handoff`
- `RunExit::Reload` → `TuiExit::Reload` → `exec_reload` in the binary

### Deliberate asymmetry

Collection is **all or nothing**: if one pty cannot cross, refuse and do not
exec. Refusing before the exec costs nothing, while a partial handoff strands
agents as unreachable orphans.

Restore is **best effort**: after the exec there is nothing to go back to, so one
bad entry must not discard agents that are still fine.

### Two orderings that matter

The engine is carried to the exec and leaked at the last moment. Dropping it
would drop every `PtyClient`, closing the masters the next image is about to
inherit.

The handoff is adopted **before** `restore_sessions`. The restore then has to
recognise what it adopted, and it does so in dux-core, so the web bootstrap gets
the same answer:

- `normalize_restored_sessions` marks a session with a live tab `Active`. It used
  to mark every row `Detached`, on the cold-boot assumption that nothing runs yet.
- `auto_reopen_candidates` skips a session with any live or launching tab. It
  used to offer every `desired_running` session, adopted or not.

The TUI's launch gate also refuses a launch over a live provider, so the TUI had
a second defense against the duplicate. The web startup sweep does not, which is
why the rule lives in core.

### File descriptors

**Every descriptor without `FD_CLOEXEC`, other than the PTY masters named in the
handoff, is a leak.** It crosses into the new image, where nothing knows it
exists: a socket that stays bound, a lock never released, a claim file held
forever. The rules:

- `prepare_for_reload` clears `FD_CLOEXEC` on the masters and nothing else.
- If collection is refused partway, or the exec fails, `Handoff::abandon` sets
  the flag again on every master already prepared.
- `ReattachedMaster::adopt` sets the flag again as soon as the new image owns
  the master, so git, gh, editors and agents spawned after a reload do not get a
  copy. The reader and writer are dups made afterwards, so they inherit it.
- Everything else dux opens is close-on-exec by default: Rust's `File`,
  `TcpListener` and `tokio` sockets all use `O_CLOEXEC`/`SOCK_CLOEXEC`. That
  covers the single-instance lock, the log, sqlite and the background web
  server's listeners.
- `Engine::pre_exec_quiesce` runs once the handoff is complete, just before the
  exec. Anything with background threads, locks or sockets that must not
  straddle the exec is wound down there (the AMQ inject workers and peer router
  hook in there when they merge).

The e2e test audits this against the kernel (`/dev/fd`), not against what the
code says it opened. Only the master may become inheritable, even with a bound
listener and an open file beside it, and after adoption nothing may be.

### The single-instance lock

`flock` locks belong to the open file description. The lock file is opened with
`O_CLOEXEC`, so the exec closes it, which releases the lock, and the new image
takes it again at startup like any cold start. If that descriptor ever survived
the exec, the new image would find the lock held by its own pid and refuse to
start. The e2e test covers exactly that: generation 1 holds the lock across the
exec, and generation 2 must be able to take it.

### When the exec fails

`exec_reload` used to `Box::leak` the engine, then `Box::from_raw` and drop it
on failure. Dropping it SIGKILLed every agent, and dux then exited. `exec` runs
no destructors, so owning the engine until the call is enough, with no leak and
no `unsafe`. On failure the handoff is abandoned (flags restored, file removed)
and the engine goes back to a resumed TUI, with the reason on the status line.

### Signals across the exec

Handlers do not survive `exec` (they reset to default), but the signal mask and
ignored dispositions do. dux blocks nothing and ignores nothing on the reload
path, and the new image registers its own handlers in `bootstrap`. Verified
live: a reloaded dux, with an adopted agent and two terminals, exits cleanly on
`SIGTERM`, the same as a cold start.

### Known cost

Scrollback is not carried. The alacritty grid lives in the old image's memory, so
a reloaded row is blank until the agent writes again. The agent itself never
stops, which is the point.

## Live verification

A release build of dux, driven in tmux with a scratch `DUX_HOME` and a
throwaway git project (2026-09-24, macOS arm64):

1. Provider `sh -c 'while true; do date; sleep 1; done'`. dux pid 33322,
   agent pid 40037. `reload-binary` was **refused** with "Not reloading: ... is
   still working", because a printing agent reads as mid-turn. That is the
   guard working. Both pids were unchanged afterwards.
2. Provider `sh -c 'echo ...; exec cat'` (idle). dux pid **53678**, agent `cat`
   pid **53689**, parent 53678. After `touch dux` and `reload-binary`:
   - dux pid still **53678**, now running `dux --reload-handoff
     .../reload-handoff-53678.json`
   - agent still **53689**, parent still **53678**, exactly one `cat`
   - log: `reload: adopted 1 of 1 handed-over ptys`
   - row shows `looper · Idle` (running), and `agent_sessions.status = active`
   - typed input echoed back through the adopted pty
   - no `reload-handoff-*.json` left in `DUX_HOME`
   - `lsof` on the new image: the lock file, and the `ptmx` master with its
     reader and writer dups
3. Opened a companion terminal (`zsh` pid 60192). Its descriptors are only its
   own tty, with no copy of the agent's master. Reloaded again: `adopted 2 of 2`,
   dux 53678, agent 53689 and terminal 60192 all unchanged. A terminal opened
   after this reload (pid 63373) got a new id beside the adopted one, and both
   shells kept running (`2 terminals`).

`tools/reload-live-check.sh [path/to/dux]` repeats checks 2 and 3 (minus the
terminal) with polling waits, and cleans up after itself. Run with the status
fix reverted, it fails on `session status is detached`.

Found along the way but not caused by reload: a dux whose host terminal
disappears (its tmux session is killed) keeps running at high CPU and ignores
`SIGTERM`. A cold start does the same, so it is outside this work.

## Test status

The reload work adds 36 tests, including an end-to-end test that execs for real
and drives a live `PtyClient` on both sides. The e2e test is a `harness = false`
test target that plays both generations itself. Before, it drove a separate
`[[bin]]` of dux-core, which `cargo install` and release builds shipped to
users.

Mutation-checked at every load-bearing point, each fails a test when broken:
- removing `keep_open_across_exec`
- dropping the slot-tab session fallback
- dropping the adopted child's exit-status memoization
- rebuilding a client around the wrong pid
- dropping companion terminal identity
- deleting the palette entry
- dropping the live-tab check from auto-reopen, or the `Active` branch from
  boot normalization
- not advancing the terminal counter past adopted ids
- not restoring `FD_CLOEXEC` on adoption, or on a refused handoff
- letting the lock descriptor survive the exec

One test was found weaker than it looked: it reported the pid straight from the
handoff, so it agreed with itself regardless of what the client was wired to. It
now reads the rebuilt client's own pid.

## Not done

- No default keybinding (palette only, deliberately: a reload clears transcripts)
- Scrollback is not preserved
- Only exercised on macOS

