# dux-amq overlay

Setup scripts that wire **dux** (the worktree TUI from `patrickdappollonio/dux`) together with **AMQ** (file-based agent-to-agent messaging from `avivsinai/agent-message-queue`) on a Linux VM with a persistent disk.

This directory does **not** modify dux source. It sits alongside the dux Rust source in this fork so I can keep both pieces under one fork while still pulling upstream.

## What you get

- **Worktree-per-agent UI** (dux) for parallel Claude/Codex/Gemini sessions
- **File-based message bus** (AMQ) so agents on the same VM can `send`/`list`/`read` between each other
- **Automatic identity**: each dux pane's AMQ handle is its git branch name, lowercased + sanitized
- **Spot-VM survival**: dux config + sessions, AMQ queue, and Claude session JSONLs all live on a persistent disk (default `/data/state/`)
- **Past-chat resume** in fresh worktrees via `--continue --fork-session` (bypasses deferred-tool blocks)
- **YOLO toggle**: `CLAUDE_YOLO=1 dux` adds `--dangerously-skip-permissions` to every pane

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

YOLO mode for that session:
```bash
CLAUDE_YOLO=1 dux
```

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

## Trade-offs

- **No native dux hook** for worktree-create lifecycle, so seeding past-chat history is done in the wrapper (one-shot, on first launch).
- **Each worktree gets its own snapshot** of past sessions on first launch (~100 MB for a heavy repo). They diverge afterward — by design.
- **Identity collisions are possible** if two worktrees normalize to the same handle. Pick distinct branch names.
- **Compaction risk**: on repos with a heavy session history, `--fork-session` inherits all of it, which can push fresh sessions toward 1M-context billing tier earlier. If that bites, set `CLAUDE_AMQ_NO_SEED=1` per-pane or revert `resume_args` to `["--continue"]`.

## Kernel compatibility (audit01 Phase 07)

`amq wake --inject-mode raw` injects message-arrival notifications via the **`TIOCSTI` ioctl** (verified by strace; see `docs/plans/audits/audit01/07-tiocsti-result.md`). This is broken on Linux 6.2+ kernels where `dev.tty.legacy_tiocsti=0` is the default — Ubuntu 24.04 and Debian 12-with-backports both ship that default.

Pick one of the following on those distros:

1. **Sysctl pin (recommended for single-user VMs):**
   ```bash
   echo 'dev.tty.legacy_tiocsti = 1' | sudo tee /etc/sysctl.d/99-amq.conf
   sudo sysctl --system
   ```
2. **External injection (no sysctl needed, no root):**
   ```bash
   amq wake --me <agent> --inject-via <bin> --inject-arg <arg>
   ```
3. **Pin AMQ to a future release** that uses `posix_openpt(3)` PTY-master writes when upstream ships it.

To verify which path your AMQ binary takes, run `dux-amq/tests/probe-amq-inject.sh` on the target host.

## Upstream sync (audit01 Phase 06)

This fork tracks `patrickdappollonio/dux@upstream/main` plus four maintained Rust patches:

| Patch | Touches |
|---|---|
| `patches/0001-clipboard-osc52.diff`        | `src/clipboard.rs` — OSC52 / wl-copy fallback |
| `patches/0002-auto-resume-on-start.diff`   | `src/app/mod.rs` — auto-resume sessions on TUI start |
| `patches/0003-scrollbar.diff`              | `src/app/render.rs` — scrollbar math + render |
| `patches/0004-config-auto-resume-field.diff` | `src/config.rs` — `auto_resume` config field |

`.github/workflows/upstream-sync.yml` runs every Sunday at 03:00 UTC, opens a **draft** PR titled `merge: upstream/main as of <sha>`, and labels it `upstream-sync`. The PR is never auto-merged; a maintainer reviews the four patched files (gated via `.github/CODEOWNERS`).

When the workflow PR conflicts on a patched file, follow the rebase recipe in `patches/README.md` and regenerate the affected diff from `HEAD`.

## License

The wrappers and scripts in this directory are MIT-licensed (matching the dux license in the parent repo).
