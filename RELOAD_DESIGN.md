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

dux has no server for agents. `PtyClient` owns the child **directly**
(`crates/dux-core/src/pty.rs`), and `shutdown_ptys_interruptible`
(`engine/lifecycle.rs:1623`) terminates every provider on exit.

## What I verified (not assumed)

| Question | Method | Result |
|---|---|---|
| Do children survive `exec`? | Compiled a Rust probe that spawns a child then execs | **Yes**, child survived, reparented to PPID 1 |
| Is the PTY master CLOEXEC? | Read `portable-pty-0.9.0/src/unix.rs:64`, then read `F_GETFD` on a live master | **Yes**, so it would be closed on exec |
| Can the master survive anyway? | Cleared `FD_CLOEXEC`, exec'd, wrote+read through the inherited fd | **Yes**, full round trip |
| Is it the *same* child? | Compared `$$` before/after exec, and read back a shell var set pre-exec | **Same pid (14309)**, in-shell state intact |

That last row is the whole feature: generation 2 drove the original live agent.
**No daemon split is required.**

## Design

Add `RunExit::Reload`, mirroring the existing `RunExit::FlipToServer`, which
already hands live `TcpListener`s across a mode switch. Same shape, same place.

Per provider, before exec:
- clear `FD_CLOEXEC` on `master.as_raw_fd()` (already exposed, used at
  `pty.rs:1840`)
- record `tab_id -> (fd, child_pid, rows, cols, spawn_dir)` into an env var or
  a handoff file

Then `exec` the new binary with a `--reload-handoff` flag. Generation 2 rebuilds
each `PtyClient` from the inherited fd.

### The one real cost

`portable-pty`'s `UnixMasterPty` has private fields and no from-fd constructor,
so the inherited fd cannot be turned back into that type. dux only needs
read / write / resize on the master, all of which work on a raw fd via `std::fs::File`
and a `TIOCSWINSZ` ioctl. So `PtyClient` needs to hold an enum: either a
portable-pty master (fresh spawn) or a reattached raw fd (post-reload).

### Scrollback

`TerminalState` (the alacritty grid) is in-process and dies with the old image.
Sessions already persist in sqlite (`storage.rs`), so tab/session mapping is
free. The grid is not. Options:
- accept a cleared scrollback on reload (agents keep running, history resets)
- serialize the visible grid to the handoff file and repaint

Start with the first; it is honest and much smaller. The agent is still live and
its next output repaints normally.

### Guards (ported from jcode)

- refuse to reload while any agent is mid-turn, unless forced
- only reload when the on-disk binary is actually newer
- if any part of the handoff fails, do not exec: stay running

## Status

Mechanism proven end to end. Next: implement `RunExit::Reload` and the fd
handoff.
