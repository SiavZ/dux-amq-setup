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

The handoff is adopted **before** `restore_sessions`, which relaunches any
session with no live provider. Adopting second would put a duplicate agent beside
every one that just survived.

### Known cost

Scrollback is not carried. The alacritty grid lives in the old image's memory, so
a reloaded row is blank until the agent writes again. The agent itself never
stops, which is the point.

## Test status

The reload work adds 30 tests, including an end-to-end test that execs for real
and drives a live `PtyClient` on both sides (passing in debug and release).

Mutation-checked at every load-bearing point, each fails a test when broken:
- removing `keep_open_across_exec`
- dropping the slot-tab session fallback
- dropping the adopted child's exit-status memoization
- rebuilding a client around the wrong pid
- dropping companion terminal identity
- deleting the palette entry

One test was found weaker than it looked: it reported the pid straight from the
handoff, so it agreed with itself regardless of what the client was wired to. It
now reads the rebuilt client's own pid.

## Not done

- No default keybinding (palette only, deliberately: a reload clears transcripts)
- Scrollback is not preserved
- Only exercised on macOS

