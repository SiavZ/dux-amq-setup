# dux-amq overlay

Setup scripts that wire **dux** (the worktree TUI from `patrickdappollonio/dux`) together with **AMQ** (file-based agent-to-agent messaging from `avivsinai/agent-message-queue`) on a Linux VM with a persistent disk.

This directory does **not** modify dux source. It sits alongside the dux Rust source in this fork so I can keep both pieces under one fork while still pulling upstream.

## What you get

- **Worktree-per-agent UI** (dux) for parallel Claude/Codex/Gemini sessions
- **File-based message bus** (AMQ) so agents on the same VM can `send`/`list`/`read` between each other
- **Automatic identity**: each dux pane's AMQ handle is the worktree directory basename, lowercased + sanitized to `[a-z0-9_-]` (typically the original branch name at worktree creation; stable across branch renames inside the worktree)
- **Spot-VM survival**: dux config + sessions, AMQ queue, and Claude session JSONLs all live on a persistent disk (default `/data/state/`)
- **Past-chat resume** in fresh worktrees via `--continue --fork-session` (bypasses deferred-tool blocks)
- **YOLO is opt-in** (audit02 P0-A): `CLAUDE_AMQ_YOLO=1` / `CODEX_AMQ_YOLO=1` enable the per-pane `--dangerously-*` flag. See [Permission model](#permission-model).
- **Per-session system prompt** (audit03 §15): the dux session-settings modal can set a custom prompt that's appended to the upstream CLI's default system prompt for one specific session — without touching `CLAUDE.md` or global config. dux exports `DUX_SYSTEM_PROMPT` in the per-PTY env at spawn time; `claude-amq` translates it into `claude --append-system-prompt <text>`. `codex-amq` and `gemini-amq` warn-and-drop because their upstream CLIs have no equivalent flag today.

## Layout

```
dux-amq/
├── install.sh                         # one-shot installer (idempotent)
├── wrappers/
│   ├── claude-amq                     # wraps `claude` with AMQ co-op + history seed
│   ├── codex-amq                      # wraps `codex` with AMQ co-op
│   └── gemini-amq                     # wraps `gemini` with AMQ co-op
├── scripts/
│   └── finalize-claude-migration.sh   # moves ~/.claude + ~/.agents onto /data
├── config/
│   ├── bashrc-additions.sh            # env vars + amq shell-setup eval
│   ├── claude-md-additions.md         # global CLAUDE.md fragment teaching AMQ usage
│   └── dux-config-changes.toml        # dux config diff to apply post-first-launch
└── vscode/
    └── settings-additions.json        # VSCode Remote-SSH terminal Ctrl-G fix
```

## Quickstart

Prerequisites:
- Linux VM with a persistent disk mounted at `/data` (tested on GCE)
- `claude` CLI on PATH (Anthropic Claude Code)
- `git`, `curl`, `tar`, `rsync`, `npx`
- `sudo` access (only for the persistent-disk migration step)

Install:
```bash
git clone https://github.com/SiavZ/dux-amq-setup.git
cd dux-amq-setup/dux-amq
./install.sh
exec bash -l
```

Optional one-time migration of an existing `~/.claude` onto `/data` (run **after** closing every running `claude` process):
```bash
/data/state/scripts/finalize-claude-migration.sh
```

Launch:
```bash
dux
```

YOLO mode for that session (legacy `CLAUDE_YOLO=1` still works for both panes):
```bash
CLAUDE_AMQ_YOLO=1 CODEX_AMQ_YOLO=1 dux
```

## Permission model

YOLO is **opt-in** as of audit02 phase 01. The wrappers default-deny on tool
execution; you must explicitly export an env var per pane to bypass prompts.
The Anthropic 2025–26 CVE wave (CVE-2025-59536, CVE-2026-21852,
CVE-2026-25723, CVE-2026-33068, CVE-2026-35020/35021/35022) all exploited
credential exfil through prompt-injected paths — default-deny is the single
biggest mitigation.

| Pane     | Env var to enable YOLO              | What it does                                         |
|----------|-------------------------------------|------------------------------------------------------|
| claude   | `CLAUDE_AMQ_YOLO=1`                 | passes `--dangerously-skip-permissions`              |
| codex    | `CODEX_AMQ_YOLO=1`                  | passes `--dangerously-bypass-approvals-and-sandbox`  |
| (legacy) | `CLAUDE_YOLO=1`                     | enables BOTH for backwards compat                    |

When YOLO is active, the wrapper prints a one-line stderr banner so it's
visible in the dux pane header. If you previously exported the deprecated
`CLAUDE_AMQ_SAFE=1` opt-out, the wrapper now prints a transitional warning;
the variable is otherwise ignored — unset it from your shell rc.

## Session seeding

Cloning the parent worktree's Claude session JSONLs into a fresh worktree
is **opt-in** (audit02 phase 01). Set `CLAUDE_AMQ_SEED_FROM_PARENT=1` to
enable.

Trade-offs to weigh before turning it on:

- **Disk amplification**: rsync clones the parent's full Claude history.
  ~100 MB per worktree on heavy repos; multiplies by N worktrees.
- **Token billing**: a long inherited history pushes new sessions toward
  the 1M-context tier earlier than a clean start would.
- **Cross-worktree info leak**: the parent's transcripts may carry secrets
  or PII from a different feature; seeding makes them readable from the
  new pane.

If you enable seeding, pair it with `resume_args = ["--resume"]` in dux
config so the picker actually shows the seeded chats. Avoid combining with
`--continue`: the latest parent session may carry a deferred-tool marker
that `--continue` refuses.

## Architecture sketch

```
┌─────────────────────────── dux (TUI on persistent disk) ───────────────────────────┐
│                                                                                    │
│  ┌──────────── pane 1: alice ───────────┐    ┌──────────── pane 2: bob ─────────┐ │
│  │   claude-amq (wrapper)               │    │   claude-amq (wrapper)           │ │
│  │     ↳ amq coop exec --me alice ─────────────┐                                │ │
│  │       ↳ claude --continue --fork-session    │   ↳ same                       │ │
│  └──────────────────────────────────────┘   ┌─┘                                  │ │
│                                             │                                    │ │
└───────────────── /data/state/amq (file-based queue) ───────────────────────────────┘
                          │
              alice's mailbox  ←→  bob's mailbox  (Maildir-style)
```

- dux creates a git worktree per pane; each pane gets its own CWD and Claude session storage.
- The `claude-amq` wrapper sets `AM_ME = <branch>`, ensures `--no-init`, and uses the shared `AMQ_GLOBAL_ROOT` queue.
- `--continue --fork-session` lets a worktree pick up the parent repo's most-recent chat as context, forking off cleanly so deferred-tool markers don't block resume.
- All inter-pane communication is `amq send <peer> "..."` from the agent — no MCP, no daemon, just files on disk.

## Kernel compatibility (TIOCSTI)

`amq wake` injects messages into agent panes via `ioctl(TIOCSTI)` by default. Linux 6.2 (Nov 2022) made `CONFIG_LEGACY_TIOCSTI` default-off, and Ubuntu 24.04 LTS / Debian 12+ ship the option built out entirely. On those kernels, `--inject-mode raw` silently fails — every wake notification is dropped before reaching the agent.

`install.sh` detects the kernel state by reading `/proc/sys/dev/tty/legacy_tiocsti`:

| Procfs value | Meaning                                | Sentinel `$STATE_ROOT/dux/.tiocsti-state` |
|--------------|----------------------------------------|-------------------------------------------|
| `1`          | TIOCSTI compiled in AND enabled        | absent (raw mode)                         |
| `0`          | Compiled in but disabled at runtime    | present (via mode)                        |
| (file absent)| Compiled out — no runtime toggle helps | present (via mode)                        |

When the sentinel is present, the wrappers switch `amq wake` to `--inject-via "$LOCAL_BIN/dux-amq-inject-bridge"`. The bridge then runs end-to-end as:

```
amq send → AMQ inbox → wake daemon → bridge → file queue → dux drainer → agent PTY
```

Each step is described in the subsections below.

### Bridge: verify mode

1. **Skip mode (default)** — transparently unwraps a `DUX1\t...` envelope when present (so legacy `amq-send-signed` callers still interop) and treats plain `amq send` bodies as raw. No HMAC check. Per the trust model in [SECURITY.md](../SECURITY.md), same-UID peers share `$HOME` and can read the HMAC secret directly, so verification doesn't add a defensible boundary against peers — insisting on it just silently dropped every legacy unsigned message in production.
2. **Strict mode (opt-in)** — set `[amq.inject].verify_envelope = true` in dux's `config.toml`. dux exports `DUX_AMQ_VERIFY=1` to spawned PTYs at bootstrap; the bridge calls `amq-receive-verify`; unsigned, replayed, stale, or MAC-mismatched envelopes are dropped silently. Outside dux, set `DUX_AMQ_VERIFY=1` directly in the shell that runs `amq wake`. Reserved for environments that genuinely cross a trust boundary (e.g. proxying wakes across hosts).

### Bridge: delivery strategy

After unwrap (or verify), the bridge picks one of three strategies for the body:

| Condition | Strategy |
|-----------|----------|
| `$DUX_PANE` set (running under dux) | Always write to file queue at `~/.local/share/dux-amq/inject-queue/<receiver>/<ts>.msg` |
| No `$DUX_PANE` but `$TMUX` set + `tmux` on PATH | `tmux send-keys -- "$body" Enter` against current pane (or `$DUX_TMUX_TARGET`) |
| Otherwise | Write to the same file queue; an operator can recover the body manually |

`<receiver>` is the sanitised `$AM_ME` exported by the wrapper (`[a-z0-9_-]` only). The wrapper derives `$AM_ME` from `basename($PWD)` of the dux worktree directory, falling back to `git branch --show-current` then `<provider>-<pid>`. When sanitisation collapses to empty, the bridge writes to the literal `_unrouted/` subdirectory; the drainer routes those messages to the currently-selected dux session with a status warning.

### Drainer (`crate::amq_inject` in the dux source)

The drainer is a tick-driven worker inside the dux process. It owns the queue and is responsible for landing each body in the right agent PTY at the right time.

- **Receiver→session mapping** (see `match_receiver` in `src/app/inject_runtime.rs`) mirrors the wrapper's identity-derivation priority: try `sanitise(basename(worktree_path))` first, then `sanitise(branch_name)`, then exact session id. This is necessary because dux sessions can change `branch_name` after worktree creation while the directory name is fixed — without basename matching, every queued message for those sessions would orphan in `.inflight`.
- **Idle detection.** Before delivering, the drainer scans the last `[amq.inject].busy_scan_lines` rows of the agent's PTY (default 5) for any of `[amq.inject].busy_markers` (default `["esc to interrupt", "ctrl+c to interrupt"]`). If a marker matches, the body stays queued. The same `InputTarget::Agent` guard the watch engine uses also applies, so a user typing in a session is never interrupted.
- **Two-phase delivery.** Once idle, the drainer types the body in **tick N** then writes a discrete `\r` in **tick N+1**. The ~16 ms gap between ticks gives Ink (Claude Code's TUI framework) time to flush the body chunk on its stdin before the `\r` arrives, so the `\r` is interpreted as a separate Enter keystroke rather than coalesced into a paste-shaped buffer that ignores it. Multi-line bodies have their interior newlines converted to Alt-Enter (`\e\r`) so they don't submit early — same chokepoint watch effects use.
- **Atomic claim.** On scan, each `<ts>.msg` file is renamed to `.inflight.<ts>.msg` before reading. Once delivered, the inflight file is unlinked. This pattern is the read-side mirror of the bridge's own `mktemp + mv -f` write pattern, and the shared `.inflight.` prefix means a single scan filter excludes both sides' in-flight files.
- **Crash recovery with expiry.** At drainer startup (see `reclaim_stale_inflight_with_max_age`), fresh `.inflight.<ts>.msg` files left behind by a prior dux instance are renamed back to `<ts>.msg`. Files older than `[amq.inject].max_message_age_secs` (default 600 seconds) are moved to the receiver's `.expired/` directory instead of being replayed into restored agents. Plain `.msg` files older than the same TTL are also expired before the startup scan, so messages accumulated while dux was offline do not flood sessions on reboot. Set the value to `0` to restore replay-all behavior. Bridge-format `mktemp .inflight.XXXXXX` files (no `.msg` suffix) are skipped on purpose — they may belong to a concurrent in-progress write.
- **Validation.** Files larger than `[amq.inject].max_message_bytes` (default 64 KiB) are rejected after claim and moved to `.rejected/` for operator inspection. Symlinks are refused. Receiver subdirectories that don't match `[a-z0-9_-]+` are rejected without a claim.
- **Polling fallback.** A 5-second polling thread (configurable via `[amq.inject].poll_interval_ms`) requests a scan in addition to the `notify` watcher, so filesystems where inotify is lossy (NFS, virtio-9p, some FUSE mounts) still drain.
- **Worker-mode postscript** (audit03 phase 01). When the receiving session has `SessionSettings.mode == Worker` (set via the dux session-settings modal), the drainer appends a sentinel-required note to the wake body before typing it: `[Orchestrator note] When this task is complete, end your reply with the literal token [task-done] so the orchestration layer knows to clean up.` The token is `crate::watch::builtin::TASK_DONE_SENTINEL`. The matching auto-clear watch rule (also opt-in per session) waits for the agent to emit `[task-done]`, then types the provider's clear command (`/clear` for claude/gemini, `/new` for codex). Postscript injection is dux-side rather than in the bash bridge — the bridge stays stateless and has no SQLite access, so the lookup of "is this receiver's session a worker?" happens at the dux drainer where `git.sessions` is in scope.

### Operator overrides

```bash
DUX_AMQ_INJECT_MODE=raw    # force TIOCSTI even on locked-down kernels
DUX_AMQ_INJECT_MODE=via    # force bridge mode (e.g. for testing)
DUX_TMUX_TARGET=<pane>     # specific tmux target for the bridge (default: current pane)
DUX_AMQ_VERIFY=1           # opt into strict HMAC verification at the bridge
```

Inspect at runtime: `cat $STATE_ROOT/dux/.tiocsti-state` (absent → raw mode active). Wake stderr lands in `~/.local/share/dux-amq/wake-<me>.log` — verify-drop reasons are visible there. Drainer activity is in dux's main JSON log under `target: "dux::amq_inject"`; grep for `delivered AMQ wake to session` for a per-message audit trail.

A native upstream fix (HMAC envelope + stdin piping inside AMQ itself) is tracked in `docs/plans/audits/audit02/artifacts/13-upstream-issue.txt`. Upstream AMQ v0.34.0 also added `--defer-while-input` / `--input-quiet-for` flags that gate TIOCSTI on terminal activity heuristics — a coarser version of what dux's drainer does with PTY-snapshot scanning.

## Trade-offs

- **No native dux hook** for worktree-create lifecycle, so seeding past-chat history (when enabled) is done in the wrapper (one-shot, on first launch).
- **Seeded worktrees get their own snapshot** of past sessions on first launch (~100 MB for a heavy repo). They diverge afterward — by design. See [Session seeding](#session-seeding) for the disk/billing/leak trade-offs.
- **Identity collisions are possible** if two worktrees normalize to the same handle. Pick distinct branch names.
- **Compaction risk** (when seeding is enabled): on repos with a heavy session history, `--fork-session` inherits all of it, which can push fresh sessions toward 1M-context billing tier earlier. If that bites, leave `CLAUDE_AMQ_SEED_FROM_PARENT` unset (the default) or revert `resume_args` to `["--continue"]`.

## Diagnostics

`dux-amq doctor` (audit02 phase 20) prints a self-check dump that
operators can attach to a support thread. Same output is reachable
via `dux doctor`, which adds a Rust-side `sessions.sqlite3` integrity
section on top of the bash-script bundle.

```bash
dux-amq doctor              # human-readable
dux-amq doctor --json       # machine-parseable; pipe to `jq`
dux-amq doctor --anonymize  # redact $HOME, branch names, agent IDs
```

It reports: AMQ binary integrity (sha256 vs the pin in
`bashrc-additions.sh`), kernel `dev.tty.legacy_tiocsti` value,
`sessions.sqlite3` `PRAGMA integrity_check`, encryption posture of
`$STATE_ROOT`, AMQ queue depth and oldest-message age, the
`~/.claude` symlink target, free disk space, and the currently-running
dux PID/uptime/RSS. Each external call is wrapped in `timeout 5`, so
the tool never hangs even when the underlying piece is broken.

The HMAC envelope (audit02 phase 08) lives at the path pointed to by
`AMQ_SECRET_PATH` (default `$HOME/.local/share/dux-amq/amq-secret`,
mode 0600). To rotate, `rm` the file and restart every pane —
rotation invalidates every in-flight signed envelope by design.
`dux-amq/scripts/amq-secret-init.sh` regenerates it idempotently on
the next install.

## Production setup

The default deployment relies on the cloud provider's at-rest
encryption (GCE PD, EBS, Azure Disk). That covers physical-disk
theft but **not** a compromised cloud IAM principal who can attach
the persistent disk to another VM and read agent transcripts /
queues in plaintext.

For stronger isolation, see the operator playbook at
[`docs/operations/encryption-at-rest.md`](../docs/operations/encryption-at-rest.md).
It covers two paths:

- **gocryptfs** — file-level FUSE encryption, no reformat, ~5% IO
  overhead. Recommended for single-user spot VMs. An opt-in helper
  is shipped at [`scripts/install-gocryptfs.sh`](scripts/install-gocryptfs.sh)
  and is **not** invoked by `install.sh`.
- **LUKS** — block-level, requires reformatting the persistent
  disk. Recommended for long-lived shared hosts.

Either path is layered on top of, not in place of, the cloud
default. See also `SECURITY.md` for the broader threat model.

## License

The wrappers and scripts in this directory are MIT-licensed (matching the dux license in the parent repo).
