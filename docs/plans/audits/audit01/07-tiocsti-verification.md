# Phase 07: TIOCSTI verification — confirm AMQ inject path

> Maps to audit findings: P1-1

## Goal
Determine whether `amq wake --inject-mode raw` uses the legacy `TIOCSTI`
ioctl (broken on Linux 6.2+ when `dev.tty.legacy_tiocsti=0`, the default on
Ubuntu 24.04 and Debian 12+) or PTY-master writes (works fine). Audit
could not introspect the binary; this phase produces the answer and ships
a kernel-pin in README, an upstream issue, or a no-op based on the result.

## Pre-conditions
- Phase 00 baseline.
- Stock Ubuntu 24.04 LTS or Debian 12 VM with `strace`, `bpftrace`, current
  `amq` on PATH.

## Files to touch
- `dux-amq/tests/probe-amq-inject.sh` — reproducible probe script.
- `dux-amq/README.md` — kernel-compat note based on result.
- `docs/plans/audits/audit01/07-tiocsti-result.md` — one-page experiment record.

## Steps
1. **Verification needed before implementation**. Probe:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   sudo sysctl dev.tty.legacy_tiocsti  # record default
   strace -f -e trace=ioctl,write -o /tmp/amq-strace.log \
     amq wake --me selftest --root /tmp/amq-probe --inject-mode raw </dev/tty &
   WPID=$!
   amq send selftest "probe-$(date +%s)"
   sleep 1; kill "$WPID" || true
   echo "TIOCSTI: $(grep -c TIOCSTI /tmp/amq-strace.log)"
   echo "PTY-master writes:"; grep -E 'write\(.*ptmx|write\(.*pts' /tmp/amq-strace.log | head
   ```
2. Interpret:
   - **TIOCSTI > 0**: broken on stock 24.04. Three options — (a) pin kernel
     min in README + require `sysctl -w dev.tty.legacy_tiocsti=1`; (b)
     file upstream issue against `avivsinai/agent-message-queue` to switch
     to PTY-master writes via `posix_openpt(3)`, with draft patch;
     (c) pin AMQ to a future fixed version.
   - **PTY-master writes only**: no version concern. Document as fine on
     `legacy_tiocsti=0`; remove worry from risk register.
3. Record result in `07-tiocsti-result.md` (date, kernel, sysctl value,
   observed syscalls, decision). Phase 17 references this.
4. README addition (template, fill per result):
   ```markdown
   ## Kernel compatibility
   `amq wake --inject-mode raw` uses <PTY-master writes | TIOCSTI ioctl>.
   <If TIOCSTI:> Set `dev.tty.legacy_tiocsti=1` via `/etc/sysctl.d/99-amq.conf`,
   OR upgrade `amq` to ≥ <pinned-tag>.
   ```
5. Add a CI smoke job running the probe on `ubuntu-24.04` runner, allowed
   to fail until step 2's option is implemented; failure is the action signal.

## Validation
- `grep -c TIOCSTI /tmp/amq-strace.log` produces a deterministic count per
  AMQ release.
- README matches observed behavior.
- Decision in `07-tiocsti-result.md` matches a Phase 17 release-gate item.

## Acceptance criteria
- [ ] Probe script reproducible on a clean GCE Ubuntu 24.04 VM.
- [ ] One-page result file with kernel, sysctl, syscalls, decision.
- [ ] README contains accurate kernel-compatibility section.
- [ ] If TIOCSTI: upstream issue filed, OR sysctl workaround documented, OR AMQ pinned to fixed version.
- [ ] CI job runs the probe weekly to catch regressions.

## References
- Audit P1-1.
- Linux 6.2 `dev.tty.legacy_tiocsti` sysctl (kernel.org tty/serial tree).
- Ubuntu LP #2046192: https://bugs.launchpad.net/ubuntu/+source/linux/+bug/2046192
- `posix_openpt(3)`: https://man7.org/linux/man-pages/man3/posix_openpt.3.html
