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
- **OSC 52 clipboard with ST fallback**: copy-from-pane works over SSH and through the VSCode terminal; export `DUX_OSC52_TERMINATOR=ST` if your terminal silently drops BEL-terminated OSC 52 sequences (rxvt, very old xterm builds)

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

## Security model

> **TL;DR**: every Claude and Codex pane runs with full filesystem and shell capability and no per-tool prompt by default. This is intentional for interactive multi-agent dev work, but it is also the largest concentrated risk on the VM — treat the host accordingly.

### Defaults

- `claude-amq` launches `claude` with `--dangerously-skip-permissions`. Set `CLAUDE_AMQ_SAFE=1` per pane to drop that flag and restore normal per-tool approval prompts.
- `codex-amq` launches `codex` with `--dangerously-bypass-approvals-and-sandbox`. Set `CODEX_AMQ_SAFE=1` per pane to drop that flag.
- `gemini-amq` does not pass any extra flag today; Gemini's own approval model applies.

### Threat model

Any prompt-injection vector that reaches a pane — a malicious `README.md`, a poisoned `gh issue` body, a tampered MCP-fetched doc, a `WebFetch`'d page, an AMQ message from a compromised peer, or a tampered git remote — can run arbitrary commands as the VM user. The blast radius includes:

- wiping `$HOME` (and, indirectly, `~/.bashrc`, `~/.ssh/`, etc.);
- exfiltrating `/data/state/amq/` (every pane's transcripts and pending messages);
- pivoting laterally into other panes' git worktrees, secrets, and `.env` files;
- pushing to remotes the user has credentials for;
- modifying this overlay's wrappers themselves to persist across reboots.

There is **no sandboxing** between panes other than POSIX file permissions; the YOLO defaults waive even prompt-time review. If a pane is compromised, assume the whole VM and every credential it holds are compromised.

### Recommended deployment

- **Ephemeral VM**. Preemptible / spot is fine — that's the model this overlay is built for. A long-lived persistent worker is **not** appropriate for these defaults; the longer the host lives, the larger the credential and history surface.
- **Encrypt the persistent disk** that holds `/data/state/`. LUKS:

  ```bash
  cryptsetup luksFormat /dev/disk/by-id/<dev>
  cryptsetup luksOpen   /dev/disk/by-id/<dev> data
  mkfs.ext4 /dev/mapper/data && mount /dev/mapper/data /data
  ```

  After unlock, `/data/state/{amq,claude,codex,gemini,dux}` lives on the encrypted volume. If the VM is destroyed without unlocking, the queue and session JSONLs are unrecoverable.
- **Keep credentials minimum-scope.** Treat the VM's `gh auth`, GCP/AWS keys, and `~/.ssh/id_*` as broadly delegable to anything that talks to a pane.
- **Audit AMQ peers**. `amq who` lists every handle that can `amq send` into your panes. A rogue peer is an injection vector.

### Data handling

Every directory below holds chat transcripts, model I/O, or other PII produced by the agents. None of it is encrypted at rest by this overlay — encryption is the operator's job (LUKS recipe above). Treat each path as "personal data, possibly special-category" for GDPR-aware deployments.

| Path                                  | Contents                                                                 | Retention                  |
|---------------------------------------|--------------------------------------------------------------------------|----------------------------|
| `~/.claude/projects/`                 | Claude session JSONLs: prompts, responses, tool I/O, secrets pasted in   | unbounded (manual delete)  |
| `/data/state/claude/`                 | Persistent symlink target for `~/.claude` (after migration)              | unbounded (manual delete)  |
| `/data/state/codex/`                  | Codex session state, tool I/O                                            | unbounded (manual delete)  |
| `/data/state/gemini/`                 | Gemini session state                                                     | unbounded (manual delete)  |
| `/data/state/agents/`                 | Per-agent scratch + AMQ identity hints                                   | unbounded (manual delete)  |
| `/data/state/amq/`                    | Maildir-style queue: every `amq send` payload between every pair of agents | unbounded; `amq` does not GC |
| `/data/state/dux/`                    | dux config + sessions DB (worktree paths, last-used providers)           | unbounded (manual delete)  |
| `/data/state/worktrees/`              | Per-pane git worktrees — full source checkouts including `.env`, secrets, build artifacts | unbounded (manual delete) |

**Retention defaults**: nothing in this overlay implements automatic rotation, redaction, or expiry. If your compliance posture needs short-lived data, add a cron job (`find /data/state/amq -mtime +N -delete`) tailored to your retention policy. AMQ messages are plain files; deleting them is sufficient.

**Encryption recipe**: see the LUKS one-liner under "Recommended deployment" above. After unlock, *every* path in the table lives on the encrypted volume. If the VM is destroyed without unlocking, all of the above is unrecoverable — which is the explicit trade-off this overlay is built around.

**Rotation hooks**: `dux-amq-doctor` (Phase 17) reports queue size and oldest-message age so you know when to prune.

### Future work

Anthropic's [Claude Code auto mode](https://www.anthropic.com/engineering/claude-code-auto-mode) (March 2026) replaces `--dangerously-skip-permissions` with classifier-gated approval. Re-point `claude-amq` at that flag once the integration story stabilises; until then the YOLO default + this section are the explicit trade-off.

### Revoking the overlay

```bash
rm -rf ~/.local/bin/{amq,dux,claude-amq,codex-amq,gemini-amq}
rm -rf /data/state/{amq,agents,claude,codex,gemini}
# rotate the LUKS passphrase / re-key the encrypted volume
cryptsetup luksChangeKey /dev/disk/by-id/<dev>
```

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
