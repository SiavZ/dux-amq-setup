# Phase 16: `dux-amq-doctor` triage tool

> Maps to audit findings: P2-11

## Goal
One command that prints every fact a support session needs: `amq` + `dux`
versions, agent registry, queue size, persistent-disk free space, kernel
`dev.tty.legacy_tiocsti` value, `~/.claude` symlink target, overlay
version, recorded binary hash vs current. Today there is no triage tool;
reproducing a user's environment over Slack takes hours.

## Pre-conditions
- Phase 13 records AMQ binary hash at `$STATE_ROOT/amq/binary.sha256`.
- Phase 12 ships `dux-amq/VERSION`.
- Phase 07 may or may not have produced its result; doctor reads live sysctl either way.

## Files to touch
- `dux-amq/bin/dux-amq-doctor` — bash script.
- `dux-amq/install.sh` — install alongside wrappers.
- `dux-amq/tests/doctor_smoke.bats` — create.

## Steps
1. Implement the doctor as a bash script with sections (Versions, Kernel,
   Persistent disk, Symlinks, AMQ, Binary integrity, Skills). Sketch of
   the load-bearing parts:
   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   STATE_ROOT="${STATE_ROOT:-/data/state}"
   section() { printf '\n\033[1;34m== %s ==\033[0m\n' "$*"; }

   # Versions: overlay (from $DUX_AMQ_LIB/../VERSION), dux --version, amq --version.
   # Kernel: uname -r, sysctl -n dev.tty.legacy_tiocsti.
   # Disk: df -h /data; du -sh per state subdir.
   # Symlinks: readlink ~/.claude ~/.agents.
   # AMQ: amq who, amq list.

   section "Binary integrity"
   if [[ -f "$STATE_ROOT/amq/binary.sha256" ]]; then
     exp=$(awk '{print $1}' "$STATE_ROOT/amq/binary.sha256")
     act=$(sha256sum "${AMQ_BIN:-/data/state/amq-bin/amq}" 2>/dev/null | awk '{print $1}')
     [[ "$exp" == "$act" ]] \
       && printf '  amq: ok (%s…)\n' "${act:0:12}" \
       || printf '  amq: MISMATCH (got %s… expected %s…)\n' "${act:0:12}" "${exp:0:12}"
   fi
   ```
2. `--json` mode using `jq -n`:
   ```bash
   if [[ "${1:-}" == "--json" ]]; then
     jq -n \
       --arg overlay "$(<"${DUX_AMQ_LIB}/../VERSION")" \
       --arg dux     "$(dux --version 2>&1)" \
       --arg kernel  "$(uname -r)" \
       --arg tiocsti "$(sysctl -n dev.tty.legacy_tiocsti 2>/dev/null)" \
       '{overlay:$overlay, dux:$dux, kernel:$kernel, tiocsti:$tiocsti}'
     exit 0
   fi
   ```
3. Install:
   ```diff
   + install -m 0755 "$HERE/bin/dux-amq-doctor" "$LOCAL_BIN/dux-amq-doctor"
   ```
4. bats smoke test: stub `amq`, `dux`, `npx` to deterministic fakes; run
   `dux-amq-doctor --json | jq .overlay`; assert it equals
   `$(< dux-amq/VERSION)`. Add a hash-mismatch case (regression for Phase 13).
5. README "Troubleshooting": "Paste `dux-amq-doctor --json` into your bug
   report."

## Validation
- `dux-amq-doctor` runs end-to-end on a real VM.
- `--json` output parses as JSON.
- bats smoke test passes against fakes.
- Hash-mismatch branch fires when AMQ binary is corrupted.

## Acceptance criteria
- [ ] `dux-amq-doctor` installed to `~/.local/bin/`. — script ships at `dux-amq/bin/dux-amq-doctor`; install.sh wiring deferred (Track D guardrail: no install.sh edits).
- [x] Outputs versions, kernel, tiocsti, disk usage, symlinks, AMQ state, hash check, skills. — verified on this VM (kernel 6.1, AMQ 0.34.0, ~/.claude → /data/state/claude). Hash-check is conditional on Phase 13's `binary.sha256` (not yet shipped) — doctor reads it when present.
- [x] `--json` parses cleanly. — `dux-amq-doctor --json | jq .` round-trips.
- [ ] Hash-mismatch branch test passes. — depends on Phase 13's sha256 file.
- [ ] README "Troubleshooting" section points at it. — deferred to Phase 17 release-gate (out of Track D scope).

## References
- Audit P2-11.
- Pattern: `kubectl version -o json`, `gh status`, `flutter doctor`.
