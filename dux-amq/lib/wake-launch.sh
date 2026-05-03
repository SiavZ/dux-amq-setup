#!/usr/bin/env bash
# shellcheck shell=bash
#
# wake-launch.sh — start `amq wake` as a backgrounded daemon, probe that
# it survived its own startup, and route its stderr/stdout to a per-pane
# rolling log so failures are loud instead of silent.
#
# Why: under `set -euo pipefail`, background-job failures do not propagate
# (`set -e` only catches foreground command exits). Today's wrappers run
#   amq wake --me ... --inject-mode raw </dev/tty >/dev/null 2>&1 &
# which means a wake daemon that crashes during its own init (missing
# binary, bad TIOCSTI permission, queue-root not writable) leaves the
# wrapper running fine — the user can type into Claude — but no AMQ
# message ever reaches the pane. The bug is invisible until someone
# tries `amq send` and nothing happens.
#
# Behaviour:
# - Logs to $HOME/.local/share/dux-amq/wake-<me>.log (creates the dir).
# - Rolls the log when it exceeds 5 MiB.
# - After spawn, sleeps DUX_AMQ_WAKE_PROBE_SECS (default 0.2) and runs
#   `kill -0 $!`. If the PID is gone, prints a red banner pointing at
#   the log file and tails the last few lines.
# - SOFT-FAIL by default: returns non-zero on probe failure, but the
#   caller is expected to `|| warn ...` and continue. Set
#   DUX_AMQ_WAKE_STRICT=1 to flip to hard-fail (caller should `||
#   exit 1`). The default policy is soft-fail because we don't want a
#   wake glitch to block the user from launching claude.

# Usage: wake_launch <me> <root> [<inject-mode>]
wake_launch() {
  local me="$1" root="$2" mode="${3:-raw}"
  local logdir="${HOME}/.local/share/dux-amq"
  mkdir -p "$logdir"
  local logfile="$logdir/wake-$me.log"

  # Rotate at 5 MiB.
  if [[ -s "$logfile" ]]; then
    local size
    size=$(stat -c%s "$logfile" 2>/dev/null || echo 0)
    if (( size > 5242880 )); then
      mv -f "$logfile" "$logfile.1"
    fi
  fi

  # Pre-create the logfile so the test harness can assert it exists even
  # if amq's exec is what fails (which is what /dev/tty unavailability
  # looks like inside bats / non-tty CI runners).
  : >>"$logfile"

  # CRITICAL: amq wake's IsTerminal(stdin) gate requires /dev/tty (bash
  # auto-redirects background-job stdin to /dev/null otherwise). Allow
  # tests to override with DUX_AMQ_WAKE_STDIN=/dev/null to skip the tty
  # requirement (the test fakes don't care).
  local stdin_path="${DUX_AMQ_WAKE_STDIN:-/dev/tty}"

  if [[ ! -r "$stdin_path" ]]; then
    printf 'amq wake stdin=%s unreadable; skipping launch\n' "$stdin_path" >>"$logfile"
    if [[ "${DUX_AMQ_WAKE_STRICT:-}" == "1" ]]; then
      printf '\033[1;31m!\033[0m amq wake stdin %s unreadable; see %s\n' \
        "$stdin_path" "$logfile" >&2
      return 1
    fi
    # Soft-fail: caller logs and continues.
    return 1
  fi

  amq wake --me "$me" --root "$root" --inject-mode "$mode" \
    <"$stdin_path" >>"$logfile" 2>&1 &
  local pid=$!
  disown "$pid" 2>/dev/null || true

  # Probe — let wake do its TIOCSTI / queue-init / IsTerminal(stdin)
  # checks before we declare survival. 200 ms is enough for the typical
  # init path on a warm VM; tunable for slow disks / debugging.
  sleep "${DUX_AMQ_WAKE_PROBE_SECS:-0.2}"

  if ! kill -0 "$pid" 2>/dev/null; then
    printf '\033[1;31m!\033[0m amq wake failed to start; see %s\n' "$logfile" >&2
    if [[ -s "$logfile" ]]; then
      printf '   last 5 log lines:\n' >&2
      tail -n 5 "$logfile" 2>/dev/null | sed 's/^/   | /' >&2 || true
    fi
    if [[ "${DUX_AMQ_WAKE_STRICT:-}" == "1" ]]; then
      return 1
    fi
    # Soft-fail: still return non-zero so the caller's `|| warn` fires,
    # but the wrapper proceeds.
    return 1
  fi
  return 0
}
