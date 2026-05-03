#!/usr/bin/env bats
#
# Phase 04: path-encoder parity + realpath containment.
#
# 1. The shared lib/path-encode.sh must agree with Claude Code's actual
#    on-disk encoding for every row in tests/fixtures/path-encoding.tsv
#    (the fixture was captured empirically against claude 2.1.111 on
#    this VM 2026-05-03; see lib/path-encode.sh for the table).
# 2. The wrappers' `ME=basename($PWD)` heuristic must reject sibling
#    paths that the previous prefix-glob accepted, e.g.
#    `$DUX_HOME/worktrees-evil/x`. It must still accept legitimate
#    `$DUX_HOME/worktrees/<name>` (and nested subdirs).

load 'lib/setup'

LIB="$BATS_TEST_DIRNAME/../lib/path-encode.sh"
FIXTURE="$BATS_TEST_DIRNAME/fixtures/path-encoding.tsv"
CLAUDE_WRAPPER="$BATS_TEST_DIRNAME/../wrappers/claude-amq"
CODEX_WRAPPER="$BATS_TEST_DIRNAME/../wrappers/codex-amq"
GEMINI_WRAPPER="$BATS_TEST_DIRNAME/../wrappers/gemini-amq"

setup() {
  setup_isolated_home
}

teardown() {
  teardown_isolated_home
}

@test "path-encode.sh sources cleanly and exposes path_encode" {
  run bash -c "source '$LIB'; type -t path_encode"
  [ "$status" -eq 0 ]
  [[ "$output" == *function* ]]
}

@test "path_encode matches every fixture row" {
  while IFS=$'\t' read -r abs expected; do
    [[ -z "$abs" || "$abs" =~ ^# ]] && continue
    actual=$(bash -c "source '$LIB'; path_encode \"$abs\"")
    if [[ "$actual" != "$expected" ]]; then
      printf 'FIXTURE MISMATCH\n  path:     %s\n  expected: %s\n  actual:   %s\n' \
        "$abs" "$expected" "$actual" >&2
      return 1
    fi
  done < "$FIXTURE"
}

# Helper: evaluate just the containment snippet that all three wrappers
# share. Inputs: $1=DUX_HOME, $2=PWD. Echoes ME (empty when rejected).
_run_containment() {
  local dh="$1" pwd_in="$2"
  bash -c '
    DUX_HOME="$1"
    cd "$2"
    ME=""
    _DUX_WTS=$(realpath -m "${DUX_HOME:-/data/state/dux}/worktrees")
    _PWD_REAL=$(realpath -m "$PWD")
    if [[ "$_PWD_REAL" == "$_DUX_WTS"/* ]]; then
      ME=$(basename "$_PWD_REAL")
    fi
    printf "%s" "$ME"
  ' _ "$dh" "$pwd_in"
}

@test "containment rejects worktrees-evil/ sibling" {
  mkdir -p "$TEST_HOME/dh/worktrees-evil/x"
  out=$(_run_containment "$TEST_HOME/dh" "$TEST_HOME/dh/worktrees-evil/x")
  # Must NOT identify as 'x' — that would be the bug.
  [[ "$out" != "x" ]]
  [[ -z "$out" ]]
}

@test "containment accepts legitimate worktrees/<name>" {
  mkdir -p "$TEST_HOME/dh/worktrees/foo"
  out=$(_run_containment "$TEST_HOME/dh" "$TEST_HOME/dh/worktrees/foo")
  [[ "$out" == "foo" ]]
}

@test "containment accepts nested worktrees/foo/bar" {
  mkdir -p "$TEST_HOME/dh/worktrees/foo/bar"
  out=$(_run_containment "$TEST_HOME/dh" "$TEST_HOME/dh/worktrees/foo/bar")
  # Basename of the resolved CWD, not of the worktree root.
  [[ "$out" == "bar" ]]
}

@test "containment rejects exact worktrees dir (no trailing path)" {
  mkdir -p "$TEST_HOME/dh/worktrees"
  out=$(_run_containment "$TEST_HOME/dh" "$TEST_HOME/dh/worktrees")
  # The boundary `/` requires a child path, not the worktrees dir itself.
  [[ -z "$out" ]]
}

@test "containment rejects worktrees-evil sibling (codex wrapper static check)" {
  grep -q 'realpath -m' "$CODEX_WRAPPER"
  run grep -F '"${DUX_HOME:-/data/state/dux}/worktrees/"*' "$CODEX_WRAPPER"
  [ "$status" -ne 0 ]
}

@test "containment block present in gemini wrapper (static check)" {
  grep -q 'realpath -m' "$GEMINI_WRAPPER"
  run grep -F '"${DUX_HOME:-/data/state/dux}/worktrees/"*' "$GEMINI_WRAPPER"
  [ "$status" -ne 0 ]
}

@test "claude wrapper sources path-encode lib and uses path_encode" {
  grep -q 'path-encode.sh' "$CLAUDE_WRAPPER"
  grep -q 'path_encode "$PWD"' "$CLAUDE_WRAPPER"
}
