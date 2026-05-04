# Phase 07 result — `amq wake` uses TIOCSTI

> Audit01 P1-1 — verification record

## Setup

| Field | Value |
|---|---|
| Date | 2026-05-03 |
| Host | GCE Debian 12 (bookworm) VM |
| Kernel | `6.1.0-45-cloud-amd64` |
| `dev.tty.legacy_tiocsti` | sysctl not present (introduced upstream in 6.2; this kernel pre-dates the gate) |
| `amq --version` | `0.34.0` |
| Command | `amq wake --me selftest --root /tmp/amq-probe --inject-mode raw` driven by `amq send --to selftest --body "probe-…"` |
| Trace tool | `strace -f -e trace=ioctl,write` |

## Observed syscalls

After delivering one short message, the strace log shows:

- **`ioctl(6, TIOCSTI, "<char>") = 0`** — **110 calls**, one ioctl per character of the injected payload.
- **`write(_, …, _)` to a `pts`/`ptmx` fd** — **0 calls** for injection (only ordinary stdout writes for the human-visible notification banner).

Full transcript: `artifacts/07-tiocsti-strace.txt`. Per-character TIOCSTI fragment:

```
ioctl(6, TIOCSTI, "A")           = 0
ioctl(6, TIOCSTI, "M")           = 0
ioctl(6, TIOCSTI, "Q")           = 0
ioctl(6, TIOCSTI, " ")           = 0
…
```

## Verdict

`amq wake --inject-mode raw` is **TIOCSTI-based**. It is **not** PTY-master writes via `posix_openpt(3)`. The audit's worry that injection might silently fail on stock Ubuntu 24.04 / Debian 12-backports kernels is **confirmed**: those ship `dev.tty.legacy_tiocsti=0` by default, which makes `ioctl(_, TIOCSTI, _)` return `EPERM`/`EINVAL` and the message-arrival notification never reaches the focused TUI.

This VM (kernel 6.1) is below the gate so it works here, but production guidance must assume newer kernels.

## Workaround (documented in dux-amq/README.md)

Three options, in order of preference for end-users:

1. **Sysctl pin** (immediate; sticky across reboots):
   ```bash
   echo 'dev.tty.legacy_tiocsti = 1' | sudo tee /etc/sysctl.d/99-amq.conf
   sudo sysctl --system
   ```
2. **External injection** — `amq wake --inject-via <bin>` bypasses TIOCSTI entirely. Use this in non-root environments where the sysctl can't be flipped.
3. **Pin AMQ to a future fixed release** — when upstream switches to PTY-master writes, drop the sysctl.

Filing an upstream issue against `avivsinai/agent-message-queue` to migrate to `posix_openpt(3)` is recommended but **not** in scope for this phase (the binary is third-party and we'd be a downstream petitioner).

## CI follow-up

Phase 17 release-gate adds an `ubuntu-24.04` job that runs `dux-amq/tests/probe-amq-inject.sh` and asserts the strace log either contains TIOCSTI **and** `legacy_tiocsti=1` is set, or contains PTY-master writes. Failure is the regression signal.
