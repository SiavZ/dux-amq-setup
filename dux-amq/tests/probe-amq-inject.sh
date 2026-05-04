#!/usr/bin/env bash
# Audit01 Phase 07 (P1-1): reproducible TIOCSTI probe for `amq wake`.
#
# What it does:
#   1. Records the kernel's `dev.tty.legacy_tiocsti` sysctl value (or notes
#      it is absent on pre-6.2 kernels).
#   2. Spawns `amq wake` under strace inside a fresh pty (script(1) gives
#      the binary the real terminal it needs to run).
#   3. Sends one message via `amq send` to that wake instance.
#   4. Counts `TIOCSTI` ioctl calls and PTY-master writes.
#   5. Prints a verdict: TIOCSTI / PTY / NEITHER.
#
# Run on a clean Ubuntu 24.04 or Debian 12 VM with `strace`, `script`, and
# `amq` on PATH. The script does not require root.
#
# Output: machine-readable summary on stdout, full strace at
# /tmp/amq-strace.log.

set -euo pipefail

ROOT="${AMQ_PROBE_ROOT:-/tmp/amq-probe}"
LOG="${AMQ_PROBE_STRACE_LOG:-/tmp/amq-strace.log}"
SCRIPTLOG="${AMQ_PROBE_SCRIPT_LOG:-/tmp/amq-wake.scriptlog}"

command -v amq >/dev/null    || { echo "amq not on PATH" >&2; exit 1; }
command -v strace >/dev/null || { echo "strace not on PATH (apt-get install strace)" >&2; exit 1; }
command -v script >/dev/null || { echo "script(1) not on PATH (bsdmainutils / util-linux)" >&2; exit 1; }

echo "## Probe: amq wake injection mechanism"
echo "Kernel: $(uname -srvm)"

if [[ -r /proc/sys/dev/tty/legacy_tiocsti ]]; then
  printf 'dev.tty.legacy_tiocsti: %s\n' "$(cat /proc/sys/dev/tty/legacy_tiocsti)"
else
  echo "dev.tty.legacy_tiocsti: (not present — kernel pre-dates 6.2 sysctl gate)"
fi

echo "amq version: $(amq --version 2>&1 | head -1)"

# Fresh AMQ root.
rm -rf "$ROOT"
amq init --root "$ROOT" --agents selftest,sender >/dev/null

# Inner script that strace will exec inside the pty.
INNER="$(mktemp -t amq-probe-inner.XXXXXX.sh)"
trap 'rm -f "$INNER"' EXIT
cat >"$INNER" <<EOF
#!/usr/bin/env bash
exec strace -f -e trace=ioctl,write -o "$LOG" -- \\
  amq wake --me selftest --root "$ROOT" --inject-mode raw
EOF
chmod +x "$INNER"

rm -f "$LOG" "$SCRIPTLOG"
script -q -c "$INNER" "$SCRIPTLOG" &
SPID=$!

# Wait for the wake to actually be running before sending.
for _ in 1 2 3 4 5 6 7 8 9 10; do
  if [[ -s "$LOG" ]]; then break; fi
  sleep 0.2
done

amq send --root "$ROOT" --me sender --to selftest --body "probe-$(date +%s)" >/dev/null
sleep 2

kill -TERM "$SPID" 2>/dev/null || true
wait "$SPID" 2>/dev/null || true

# Tally.
tiocsti_count=$(grep -c TIOCSTI "$LOG" 2>/dev/null || echo 0)
pty_writes=$(grep -cE 'write\([0-9]+, .*, [0-9]+\) = [0-9]+ /dev/(pts|ptmx)' "$LOG" 2>/dev/null || echo 0)
# PTY-master writes are usually surfaced as plain write() syscalls; the
# above pattern is a best-effort heuristic.

echo
echo "## Tally"
echo "TIOCSTI ioctls: $tiocsti_count"
echo "write() to pty (heuristic): $pty_writes"

echo
echo "## Verdict"
if (( tiocsti_count > 0 )); then
  echo "VERDICT: TIOCSTI"
  echo "  Workaround: set dev.tty.legacy_tiocsti=1 (see dux-amq/README.md)"
  echo "  OR use --inject-via for external injection."
elif (( pty_writes > 0 )); then
  echo "VERDICT: PTY-MASTER-WRITES"
  echo "  No kernel sysctl pin needed. Safe on legacy_tiocsti=0."
else
  echo "VERDICT: NEITHER (probe likely failed; inspect $LOG)"
  exit 2
fi
