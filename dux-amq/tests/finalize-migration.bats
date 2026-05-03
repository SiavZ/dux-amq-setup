#!/usr/bin/env bats
#
# Phase 11 (audit02) — finalize-claude-migration.sh hardening.
#
# Covers acceptance criteria from
# docs/plans/audits/audit02/11-migration-safety.md:
#
#   1. Aborts when claude / claude-amq is running.
#   2. Idempotent — re-run on a finalised tree is a no-op.
#   3. Default rsync preserves pre-existing files at the destination.
#   4. FINALIZE_FORCE_DELETE=1 honours the legacy destructive behaviour.
#   5. Two parallel invocations: one wins, one fails fast (flock).
#
# All tests use an isolated $HOME and a private bin dir prepended to PATH
# so the script's `pgrep` and `rsync` calls hit our fakes, not the real
# binaries. The fakes are tiny shell shims keyed off env vars set by the
# test, which keeps each scenario self-contained and easy to read.

load 'lib/setup'

# Path to the script under test, resolved relative to this .bats file.
# `realpath` collapses `..` and confirms the script exists.
SCRIPT="$(realpath "$BATS_TEST_DIRNAME/../scripts/finalize-claude-migration.sh")"

setup() {
  setup_isolated_home

  # Some assertions need to know where the persistent-disk root lives.
  # We can't use /data/state/ in tests (host-owned, may not exist), so we
  # redirect everything under TEST_HOME/state/ and patch the script via
  # an HOME=$TEST_HOME shim later. The script hardcodes /data/state — so
  # we instead bind-mount-style redirect by running the script with a
  # `--root` shim. Simpler: copy the script to a temp file with the path
  # rewritten. This keeps the production script untouched.
  STATE_ROOT="$TEST_HOME/state"
  export STATE_ROOT
  mkdir -p "$STATE_ROOT"

  # Build a per-test copy of the script with /data/state replaced by
  # $STATE_ROOT and /tmp/dux-amq-finalize.lock replaced by a per-test
  # lock file. This is the cleanest way to test without modifying the
  # production script's filesystem assumptions.
  TEST_SCRIPT="$TEST_HOME/finalize.sh"
  LOCK_FILE="$TEST_HOME/finalize.lock"
  export LOCK_FILE
  sed \
    -e "s|/data/state|$STATE_ROOT|g" \
    -e "s|/tmp/dux-amq-finalize.lock|$LOCK_FILE|g" \
    "$SCRIPT" > "$TEST_SCRIPT"
  chmod +x "$TEST_SCRIPT"

  # Per-test fake bin dir, ahead of tests/fakes (which is empty) and the
  # real PATH. Each test populates `pgrep`, `rsync` shims as needed.
  FAKE_BIN="$TEST_HOME/bin"
  mkdir -p "$FAKE_BIN"
  export PATH="$FAKE_BIN:$PATH"
}

teardown() {
  teardown_isolated_home
}

# Drop a fake `pgrep` that always returns the supplied exit code. The
# script's `pgrep -x 'claude(-amq)?'` invocation will hit this shim
# because $FAKE_BIN is first on PATH.
fake_pgrep() {
  local exit_code="$1"
  cat > "$FAKE_BIN/pgrep" <<EOF
#!/bin/sh
# Fake pgrep used by phase 11 tests. Always returns exit $exit_code so
# tests can simulate "claude is running" / "claude is not running"
# without depending on the host process table.
exit $exit_code
EOF
  chmod +x "$FAKE_BIN/pgrep"
}

# Drop a fake `rsync` that just records its argv into a log file inside
# $TEST_HOME. Tests can then assert on whether `--delete` was passed.
# Real rsync semantics aren't needed: the file copy is exercised
# indirectly by checking what the script asked rsync to do.
fake_rsync() {
  cat > "$FAKE_BIN/rsync" <<EOF
#!/bin/sh
# Fake rsync used by phase 11 tests. Records argv to \$TEST_HOME/rsync.log
# and additively copies SRC/* to DST/ so subsequent assertions about
# preserved files at DST hold even though we are not invoking real rsync.
printf '%s\\n' "\$*" >> "$TEST_HOME/rsync.log"
src=""
dst=""
for a in "\$@"; do
  case "\$a" in
    -*) ;;
    *) if [ -z "\$src" ]; then src="\$a"; else dst="\$a"; fi ;;
  esac
done
[ -n "\$src" ] && [ -n "\$dst" ] && cp -aR "\$src". "\$dst" 2>/dev/null || true
exit 0
EOF
  chmod +x "$FAKE_BIN/rsync"
}

# -----------------------------------------------------------------------
# Test 1: claude running -> abort.
#
# When pgrep reports a live claude (exit 0), the script must refuse to
# touch anything. We don't need rsync for this one because the abort
# happens before the first migrate_dir call.
# -----------------------------------------------------------------------
@test "finalize aborts when claude is running" {
  fake_pgrep 0       # pgrep "found claude"
  fake_rsync         # never called, but stub just in case

  mkdir -p "$HOME/.claude" "$HOME/.agents"
  : > "$HOME/.claude/marker"

  run "$TEST_SCRIPT"
  [ "$status" -ne 0 ]
  echo "$output" | grep -qi 'claude.*running'

  # Source dir must be untouched.
  [ -d "$HOME/.claude" ]
  [ -f "$HOME/.claude/marker" ]
  [ ! -L "$HOME/.claude" ]
}

# -----------------------------------------------------------------------
# Test 2: idempotency.
#
# After a successful first run, ~/.claude is a symlink. A second run
# must short-circuit on the `[[ -L "$src" ]]` branch and NOT touch the
# destination, NOT call rsync, NOT create a new backup.
# -----------------------------------------------------------------------
@test "finalize is idempotent on a finalised tree" {
  fake_pgrep 1       # claude not running
  fake_rsync

  mkdir -p "$HOME/.claude"
  : > "$HOME/.claude/seed"
  mkdir -p "$HOME/.agents"
  : > "$HOME/.agents/seed"

  run "$TEST_SCRIPT"
  [ "$status" -eq 0 ]
  [ -L "$HOME/.claude" ]
  [ -L "$HOME/.agents" ]

  # Second run: rsync.log should not gain new entries because the script
  # short-circuits before calling rsync. Snapshot then compare.
  cp "$TEST_HOME/rsync.log" "$TEST_HOME/rsync.log.snap" 2>/dev/null || true
  run "$TEST_SCRIPT"
  [ "$status" -eq 0 ]
  if [ -f "$TEST_HOME/rsync.log" ] && [ -f "$TEST_HOME/rsync.log.snap" ]; then
    diff -q "$TEST_HOME/rsync.log" "$TEST_HOME/rsync.log.snap"
  fi
  echo "$output" | grep -q 'already a symlink'
}

# -----------------------------------------------------------------------
# Test 3: pre-existing destination files survive by default.
#
# Default rsync invocation must NOT pass --delete. We pre-populate
# $STATE_ROOT/claude/preserved.txt and assert it survives. We also
# inspect rsync.log to confirm --delete was absent.
# -----------------------------------------------------------------------
@test "finalize preserves pre-existing files at destination by default" {
  fake_pgrep 1
  fake_rsync

  mkdir -p "$HOME/.claude"
  : > "$HOME/.claude/seed"
  mkdir -p "$STATE_ROOT/claude"
  echo "keep me" > "$STATE_ROOT/claude/preserved.txt"

  mkdir -p "$HOME/.agents"
  : > "$HOME/.agents/seed"

  run "$TEST_SCRIPT"
  [ "$status" -eq 0 ]
  [ -f "$STATE_ROOT/claude/preserved.txt" ]
  grep -q 'keep me' "$STATE_ROOT/claude/preserved.txt"

  # Confirm rsync was invoked WITHOUT --delete on the claude directory.
  ! grep -- '--delete' "$TEST_HOME/rsync.log"
}

# -----------------------------------------------------------------------
# Test 4: FINALIZE_FORCE_DELETE=1 opts back into destructive sync.
#
# The opt-in must reach rsync as `--delete`. We don't need the file to
# actually disappear (our fake rsync is additive); the assertion is on
# the argv passed.
# -----------------------------------------------------------------------
@test "finalize honors FINALIZE_FORCE_DELETE=1" {
  fake_pgrep 1
  fake_rsync

  mkdir -p "$HOME/.claude"
  : > "$HOME/.claude/seed"
  mkdir -p "$HOME/.agents"
  : > "$HOME/.agents/seed"

  FINALIZE_FORCE_DELETE=1 run "$TEST_SCRIPT"
  [ "$status" -eq 0 ]
  grep -- '--delete' "$TEST_HOME/rsync.log"
}

# -----------------------------------------------------------------------
# Test 5: two parallel invocations -> one wins, one fails fast.
#
# We hold the lock from the test process for the duration of the second
# script's execution. flock -n in the script should exit non-zero with
# a "another instance" message.
# -----------------------------------------------------------------------
@test "two parallel finalize invocations: one wins, one fails fast" {
  fake_pgrep 1
  fake_rsync

  mkdir -p "$HOME/.claude" "$HOME/.agents"

  # Acquire the lock from the test shell on fd 8, then run the script.
  # The script's `flock -n 9` on the same path will fail because the
  # underlying file is already locked by us.
  exec 8>"$LOCK_FILE"
  flock -n 8

  run "$TEST_SCRIPT"
  [ "$status" -ne 0 ]
  echo "$output" | grep -qi 'another instance'

  # Release our lock; the next run should succeed.
  exec 8>&-

  run "$TEST_SCRIPT"
  [ "$status" -eq 0 ]
}
