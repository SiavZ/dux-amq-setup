# dux-amq overlay

Setup scripts that wire **dux** (the worktree TUI from `patrickdappollonio/dux`) together with **AMQ** (file-based agent-to-agent messaging from `avivsinai/agent-message-queue`) on a Linux VM with a persistent disk.

This directory does **not** modify dux source. It sits alongside the dux Rust source in this fork so I can keep both pieces under one fork while still pulling upstream.

## What you get

- **Worktree-per-agent UI** (dux) for parallel Claude/Codex/Gemini sessions
- **File-based message bus** (AMQ) so agents on the same VM can `send`/`list`/`read` between each other
- **Automatic identity**: each dux pane's AMQ handle is its git branch name, lowercased + sanitized
- **Spot-VM survival**: dux config + sessions, AMQ queue, and Claude session JSONLs all live on a persistent disk (default `/data/state/`)
- **Optional past-chat seeding** in fresh worktrees: opt-in with `CLAUDE_AMQ_SEED_FROM_PARENT=1`, then `resume_args = ["--resume"]` lets the picker browse copied history
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

- **Session-history seeding is OFF by default.** Set `CLAUDE_AMQ_SEED_FROM_PARENT=1` (per-pane env var) to copy the parent repo's Claude session JSONLs into a fresh worktree on first launch. Pair with `resume_args = ["--resume"]` in `~/.config/dux/config.toml` so the resume picker can browse the seeded history. Avoid combining with `--continue`: the latest parent session may carry a deferred-tool marker that `--continue` refuses.
- **Migrating from earlier versions**: if you previously exported `CLAUDE_AMQ_NO_SEED=1`, drop it (the new default already skips seeding). To preserve the old default-on behavior, export `CLAUDE_AMQ_SEED_FROM_PARENT=1` and revisit `resume_args` in `config.toml`.
- **No native dux hook** for worktree-create lifecycle, so seeding (when opted in) happens in the wrapper one-shot on first launch.
- **Each opted-in worktree gets its own snapshot** of past sessions on first launch (~100 MB for a heavy repo). They diverge afterward — by design.
- **Identity collisions are possible** if two worktrees normalize to the same handle. Pick distinct branch names.
- **Compaction risk** when seeding is enabled: forked sessions inherit the parent's full history, which can push fresh sessions toward the 1M-context billing tier earlier. If that bites, just leave seeding off (the default).

## License

The wrappers and scripts in this directory are MIT-licensed (matching the dux license in the parent repo).
