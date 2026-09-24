#!/usr/bin/env bats
#
# audit02 phase 12: path-encoder fixtures + is_dux_worktree containment.
#
# Verifies that:
#   1. dux-amq/scripts/encode-claude-project-dir produces the same output
#      as Claude Code itself for every input/expected pair recorded in
#      tests/fixtures/claude-paths.txt. Pairs were captured from a probe
#      against a live `claude --print` install — see fixture file header.
#   2. The is_dux_worktree() helper inlined in claude-amq / codex-amq /
#      gemini-amq rejects sibling-prefix paths (the audit01 P0-5 bug:
#      `…/worktrees-evil/x` used to match the worktrees prefix glob and
#      hand the AMQ identity over to a non-dux directory).
#   3. is_dux_worktree() accepts genuine `…/worktrees/<name>` paths.
#
# The is_dux_worktree() helper is identical in all three wrappers (any
# divergence is itself a regression). We re-source one of them and let
# the other two ride on the same logic — the wrappers test (wrappers.bats)
# already proves identity flows through to the recorded amq argv.

load 'lib/setup'

ENC_SCRIPT="$BATS_TEST_DIRNAME/../scripts/encode-claude-project-dir"
FIXTURES="$BATS_TEST_DIRNAME/fixtures/claude-paths.txt"
WRAPPERS_DIR="$BATS_TEST_DIRNAME/../wrappers"

setup() {
  setup_isolated_home
}

teardown() {
  teardown_isolated_home
}

@test "encode-claude-project-dir matches recorded fixtures" {
  [ -x "$ENC_SCRIPT" ] || { echo "encoder script missing or not executable: $ENC_SCRIPT" >&2; return 1; }
  [ -f "$FIXTURES" ]    || { echo "fixture file missing: $FIXTURES" >&2; return 1; }

  local count=0 fail=0
  # IFS=$'\t' so a single TAB separates input from expected. Comments
  # and blank lines are skipped — keep that in sync with the fixture
  # file header.
  while IFS=$'\t' read -r input expected; do
    [[ -z "$input" || "${input:0:1}" == "#" ]] && continue
    count=$((count+1))
    local actual
    actual=$("$ENC_SCRIPT" -- "$input")
    if [[ "$actual" != "$expected" ]]; then
      printf 'FAIL: encode(%q) = %q, expected %q\n' "$input" "$actual" "$expected" >&2
      fail=$((fail+1))
    fi
  done < "$FIXTURES"

  if (( count < 6 )); then
    printf 'fixture coverage too low: %d entries (need >= 6)\n' "$count" >&2
    return 1
  fi
  if (( fail != 0 )); then
    printf '%d fixture mismatches out of %d\n' "$fail" "$count" >&2
    return 1
  fi
}

@test "encoder rejects relative path" {
  run "$ENC_SCRIPT" -- "relative/path"
  [ "$status" -eq 2 ]
  [[ "$output" == *"absolute path required"* ]] || {
    printf 'expected absolute-path error, got: %s\n' "$output" >&2
    return 1
  }
}

@test "encoder strips a single trailing slash" {
  local out
  out=$("$ENC_SCRIPT" -- "/foo/bar/")
  [[ "$out" == "-foo-bar" ]] || { echo "got: $out" >&2; return 1; }
}

@test "encoder preserves case (Claude does not lowercase)" {
  local out
  out=$("$ENC_SCRIPT" -- "/Foo/BarBaz")
  [[ "$out" == "-Foo-BarBaz" ]] || { echo "got: $out" >&2; return 1; }
}

# --- is_dux_worktree containment -------------------------------------------
#
# The helper is defined inline in each wrapper. We exercise it by
# sourcing claude-amq with a wrapped early-return that dumps the
# function's exit status and bails before any real work runs. The
# alternative — duplicating the helper into a shared script and loading
# it directly — was considered, but the wrappers must not develop a
# runtime fan-out beyond the wrapper itself: a separate sourced helper
# becomes a third file users must keep in sync. Test-side, we extract
# via grep; if the helper definition shifts shape the test will fail
# loudly here rather than silently passing.

# Pull the function definition out of claude-amq so we can unit-test
# it without spawning the full wrapper (which would also try to call
# git, amq wake, etc.).
source_is_dux_worktree() {
  local wrapper="$WRAPPERS_DIR/claude-amq"
  # Match `is_dux_worktree() { ... }` (single-level braces — the helper
  # itself has no nested function definitions).
  awk '
    /^is_dux_worktree\(\) \{$/ { capture = 1 }
    capture                    { print }
    capture && /^\}$/          { exit }
  ' "$wrapper" > "$TEST_HOME/helper.sh"
  [ -s "$TEST_HOME/helper.sh" ] || {
    echo "could not extract is_dux_worktree from $wrapper" >&2
    return 1
  }
  # shellcheck disable=SC1091
  source "$TEST_HOME/helper.sh"
}

@test "is_dux_worktree rejects sibling /worktrees-evil prefix" {
  source_is_dux_worktree
  mkdir -p "$TEST_HOME/dux-state/worktrees-evil/x"
  mkdir -p "$TEST_HOME/dux-state/worktrees"
  cd "$TEST_HOME/dux-state/worktrees-evil/x"
  DUX_HOME="$TEST_HOME/dux-state" run is_dux_worktree
  [ "$status" -ne 0 ] || {
    echo "is_dux_worktree wrongly accepted $PWD" >&2
    return 1
  }
}

@test "is_dux_worktree accepts /worktrees/x" {
  source_is_dux_worktree
  mkdir -p "$TEST_HOME/dux-state/worktrees/x"
  cd "$TEST_HOME/dux-state/worktrees/x"
  DUX_HOME="$TEST_HOME/dux-state" run is_dux_worktree
  [ "$status" -eq 0 ] || {
    echo "is_dux_worktree wrongly rejected $PWD" >&2
    return 1
  }
}

@test "is_dux_worktree rejects when DUX_HOME does not exist" {
  source_is_dux_worktree
  mkdir -p "$TEST_HOME/cwd"
  cd "$TEST_HOME/cwd"
  DUX_HOME="$TEST_HOME/nope" run is_dux_worktree
  [ "$status" -ne 0 ]
}

@test "is_dux_worktree definition is byte-identical across all three wrappers" {
  # Any divergence is itself a regression. Extract the helper from each
  # wrapper, compare the bodies. If they ever drift, this test fails
  # loudly long before a user discovers the asymmetry in the field.
  local extract_one
  extract_one() {
    awk '
      /^is_dux_worktree\(\) \{$/ { capture = 1 }
      capture                    { print }
      capture && /^\}$/          { exit }
    ' "$1"
  }
  local c x g
  c=$(extract_one "$WRAPPERS_DIR/claude-amq")
  x=$(extract_one "$WRAPPERS_DIR/codex-amq")
  g=$(extract_one "$WRAPPERS_DIR/gemini-amq")
  [[ -n "$c" ]] || { echo "claude-amq helper missing" >&2; return 1; }
  [[ "$c" == "$x" ]] || {
    echo "claude-amq vs codex-amq is_dux_worktree differ:" >&2
    diff <(printf '%s\n' "$c") <(printf '%s\n' "$x") >&2 || true
    return 1
  }
  [[ "$c" == "$g" ]] || {
    echo "claude-amq vs gemini-amq is_dux_worktree differ:" >&2
    diff <(printf '%s\n' "$c") <(printf '%s\n' "$g") >&2 || true
    return 1
  }
}
