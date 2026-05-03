# Phase 08: Wake-daemon startup probe + visible logs

> Maps to audit findings: P1-2

## Goal
Make `amq wake` startup failures loud. The three wrappers spawn it as a
background job under `set -euo pipefail` with stderr to `/dev/null`.
`set -e` does not propagate background-job failures; a wake daemon that
crashes during init produces a wrapper that runs fine, accepts input, and
silently never delivers AMQ messages on the receive side.

## Pre-conditions
- Phase 07 result recorded (so we know whether wake's failure mode is
  TIOCSTI-related or transport-related).

## Files to touch
- `dux-amq/lib/wake-launch.sh` — shared launcher with probe.
- `dux-amq/wrappers/{claude,codex,gemini}-amq` — call the helper.
- `dux-amq/tests/wake_probe.bats` — create.

## Steps
1. Centralise the launcher. (a) per-pane log; (b) `disown`; (c) `kill -0`
   probe after a tunable sleep to confirm survival:
   ```bash
   # dux-amq/lib/wake-launch.sh
   wake_launch() {
     local me="$1" root="$2" mode="${3:-raw}"
     local logdir="$HOME/.local/share/dux-amq"; mkdir -p "$logdir"
     local logfile="$logdir/wake-$me.log"
     # rotate at 5 MiB
     [[ -s "$logfile" && $(stat -c%s "$logfile") -gt 5242880 ]] && mv "$logfile" "$logfile.1"
     exec 3>>"$logfile"
     amq wake --me "$me" --root "$root" --inject-mode "$mode" </dev/tty >&3 2>&3 &
     local pid=$!
     disown "$pid" 2>/dev/null || true
     sleep "${DUX_AMQ_WAKE_PROBE_SECS:-0.2}"
     if ! kill -0 "$pid" 2>/dev/null; then
       printf '\033[1;31m!\033[0m amq wake failed; see %s\n' "$logfile" >&2
       tail -n 5 "$logfile" >&2 || true
       return 1
     fi
   }
   ```
2. Replace the inline launch in each wrapper:
   ```diff
   - amq wake --me "$ME" --root "$ROOT" --inject-mode raw </dev/tty >/dev/null 2>&1 &
   + source "$DUX_AMQ_LIB/wake-launch.sh"
   + wake_launch "$ME" "$ROOT" raw || warn "continuing without wake"
   ```
   `gemini-amq` passes `auto`. Default policy soft-fail (warn + continue);
   `DUX_AMQ_WAKE_STRICT=1` flips to hard-fail.
3. bats: replace `amq` on PATH with a fake that exits 1 immediately;
   assert `wake_launch` returns non-zero and the log is non-empty. Then a
   fake that sleeps; assert return 0 and `kill -0 $!` succeeds.

## Validation
- `bats dux-amq/tests/wake_probe.bats` covers failure-path and success-path.
- Manual: `mv ~/.local/bin/amq{,.bak}`; new pane shows red banner and the
  log file contains a real error.
- `tail -f ~/.local/share/dux-amq/wake-<me>.log` shows real activity, not silenced.

## Acceptance criteria
- [ ] No `>/dev/null 2>&1` on `amq wake` anywhere.
- [ ] Failed startup → red banner pointing at log file.
- [ ] `kill -0` probe in the launcher; sleep tunable via env.
- [ ] Log rotates at 5 MiB.
- [ ] bats covers both paths.

## References
- Audit P1-2.
- `set -e` and background jobs: https://www.gnu.org/software/bash/manual/html_node/The-Set-Builtin.html
- `disown`: https://www.gnu.org/software/bash/manual/html_node/Job-Control-Builtins.html
