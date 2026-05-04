#!/usr/bin/env bats
#
# audit02 Phase 13 (audit01 P1-1): kernel-state TIOCSTI detection.
#
# Validates the `tiocsti_status` helper inside `dux-amq/install.sh`.
# We don't run the whole installer (it requires /data, network, sudo);
# we extract the function via `sed` and source it in isolation. The
# helper takes its procfs path from `$TIOCSTI_PROC_PATH` so tests can
# point it at a fake file under $TEST_HOME.
#
# Returns:
#   0 — sysctl/procfs reports `1` (TIOCSTI usable)
#   1 — sysctl/procfs reports `0` (compiled in but disabled)
#   2 — file absent / unreadable / unrecognised value (compiled out)
#
# Why test this in isolation: the consequences of getting it wrong are
# silent (wake notifications dropped); a unit test pinned against a
# fake procfs file is the cheapest way to catch regressions.

load 'lib/setup'

INSTALL_SH="$BATS_TEST_DIRNAME/../install.sh"

setup() {
  setup_isolated_home
  FAKE_PROC="$TEST_HOME/legacy_tiocsti"
  export TIOCSTI_PROC_PATH="$FAKE_PROC"

  # Extract `tiocsti_status` from install.sh. The function is delimited
  # by `tiocsti_status() {` and the matching closing `}` at column 0.
  # `awk` is a hard install.sh prereq, so we lean on it here too.
  EXTRACTED="$TEST_HOME/tiocsti.sh"
  awk '
    /^tiocsti_status\(\) \{/ { capture=1 }
    capture                  { print }
    capture && /^\}$/        { capture=0; exit }
  ' "$INSTALL_SH" >"$EXTRACTED"

  # Sanity-check the extraction so a typo in install.sh fails the test
  # rather than silently neutering the assertions below.
  grep -q 'TIOCSTI_PROC_PATH' "$EXTRACTED"
  grep -q 'return 2' "$EXTRACTED"
}

teardown() {
  teardown_isolated_home
}

# Tiny harness: source the extracted function, call it, return its rc.
run_status() {
  bash -c "set -e; source '$EXTRACTED'; tiocsti_status; echo rc=\$?" 2>&1 \
    || true
  # `set -e` would abort on rc 1/2; capture explicitly via `|| true`.
}

@test "tiocsti_status: file absent → rc=2 (compiled-out kernel)" {
  rm -f "$FAKE_PROC"
  run bash -c "source '$EXTRACTED'; tiocsti_status"
  [ "$status" -eq 2 ]
}

@test "tiocsti_status: file=1 → rc=0 (TIOCSTI usable)" {
  printf '1\n' >"$FAKE_PROC"
  run bash -c "source '$EXTRACTED'; tiocsti_status"
  [ "$status" -eq 0 ]
}

@test "tiocsti_status: file=0 → rc=1 (compiled in but disabled)" {
  printf '0\n' >"$FAKE_PROC"
  run bash -c "source '$EXTRACTED'; tiocsti_status"
  [ "$status" -eq 1 ]
}

@test "tiocsti_status: file=garbage → rc=2 (treat as compiled-out)" {
  printf 'banana\n' >"$FAKE_PROC"
  run bash -c "source '$EXTRACTED'; tiocsti_status"
  [ "$status" -eq 2 ]
}

@test "tiocsti_status: empty file → rc=2 (treat as compiled-out)" {
  : >"$FAKE_PROC"
  run bash -c "source '$EXTRACTED'; tiocsti_status"
  [ "$status" -eq 2 ]
}

# Smoke: install.sh references `$STATE_ROOT/dux/.tiocsti-state` exactly
# in the two expected places (write on disabled, rm -f on enabled).
# Catches accidental rename of the sentinel.
@test "install.sh writes and removes \$STATE_ROOT/dux/.tiocsti-state" {
  grep -q 'tiocsti_disabled' "$INSTALL_SH"
  grep -q '\.tiocsti-state' "$INSTALL_SH"
  # Both paths must be present (one to write, one to clear).
  local writes clears
  writes=$(grep -c "printf 'tiocsti_disabled" "$INSTALL_SH" || true)
  clears=$(grep -c 'rm -f "\$TIOCSTI_FLAG"' "$INSTALL_SH" || true)
  [ "$writes" -ge 1 ]
  [ "$clears" -ge 1 ]
}
