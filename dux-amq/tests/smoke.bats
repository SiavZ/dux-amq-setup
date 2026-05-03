#!/usr/bin/env bats
#
# Phase 00 smoke test: verifies `lib/setup.bash` sources cleanly and
# its helpers behave as documented. Phases 02+ will add their own
# .bats files alongside this one.

load 'lib/setup'

setup() {
  setup_isolated_home
}

teardown() {
  teardown_isolated_home
}

@test "setup_isolated_home creates a fresh \$HOME under /tmp" {
  [ -n "$TEST_HOME" ]
  [ -d "$TEST_HOME" ]
  [ "$HOME" = "$TEST_HOME" ]
  case "$TEST_HOME" in
    /tmp/*|/var/folders/*|"$TMPDIR"*) ;;
    *) printf 'TEST_HOME=%s is not under a tmp prefix\n' "$TEST_HOME" >&2; return 1 ;;
  esac
}

@test "setup_isolated_home prepends tests/fakes to PATH" {
  case "$PATH" in
    "$BATS_TEST_DIRNAME/fakes":*) ;;
    *) printf 'PATH does not start with fakes/: %s\n' "$PATH" >&2; return 1 ;;
  esac
}

@test "teardown_isolated_home removes \$TEST_HOME and is idempotent" {
  local saved="$TEST_HOME"
  teardown_isolated_home
  [ ! -d "$saved" ]
  [ -z "${TEST_HOME:-}" ]
  # Second call must not error.
  teardown_isolated_home
}
