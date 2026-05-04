#!/usr/bin/env bash
# shellcheck shell=bash
#
# Common bats helpers for the dux-amq overlay test suite.
#
# Phases 02 (claude-amq seed default), 03 (finalize migration safety),
# 04 (path encoding), and 12 (versioned config inserts) all depend on
# these helpers. Keep them small, side-effect free, and idempotent.
#
# Usage from a .bats file:
#
#   load 'lib/setup'
#
#   setup() { setup_isolated_home; }
#   teardown() { teardown_isolated_home; }
#
# `$BATS_TEST_DIRNAME` is set by bats to the directory containing the
# currently-running .bats file. Fakes live alongside as
# `dux-amq/tests/fakes/`.

# Create a throwaway $HOME under /tmp so tests cannot mutate the real
# user dotfiles. Also prepends `tests/fakes/` to PATH so individual
# tests can shadow `git`, `claude`, `amq`, etc. by dropping an
# executable file in there.
setup_isolated_home() {
  TEST_HOME="$(mktemp -d -t dux-amq-test.XXXXXX)"
  export TEST_HOME
  export HOME="$TEST_HOME"
  # Phase 12: prepend `dux-amq/scripts` so `encode-claude-project-dir`
  # is findable by wrappers under test, mirroring the post-install
  # state where install.sh has placed it on $LOCAL_BIN.
  export PATH="$BATS_TEST_DIRNAME/fakes:$BATS_TEST_DIRNAME/../scripts:$PATH"
}

# Remove the throwaway $HOME. Safe to call when setup_isolated_home was
# never called (TEST_HOME unset → no-op).
teardown_isolated_home() {
  if [[ -n "${TEST_HOME:-}" && -d "$TEST_HOME" ]]; then
    rm -rf "$TEST_HOME"
  fi
  unset TEST_HOME
}
