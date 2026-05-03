#!/usr/bin/env bats
#
# Phase 02: seeding-default-flip regression coverage.
#
# Asserts the wrapper:
#   - is sourceable (Phase 02 sourcing guard) without exec'ing claude;
#   - skips seeding by default;
#   - seeds when CLAUDE_AMQ_SEED_FROM_PARENT=1 and the parent project dir
#     has .jsonl files;
#   - no longer references CLAUDE_AMQ_NO_SEED in code (README migration
#     bullet still names it on purpose).

load 'lib/setup'

WRAPPER="$BATS_TEST_DIRNAME/../wrappers/claude-amq"

setup() {
  setup_isolated_home

  # Build a minimal fake-git driver that gives the wrapper everything it
  # needs to take the seeding code path. The wrapper passes -C <dir> to
  # every git invocation, so peel that prefix off.
  PARENT="$TEST_HOME/parent"
  CHILD="$TEST_HOME/child"
  mkdir -p "$PARENT" "$CHILD"
  cat >"$BATS_TEST_DIRNAME/fakes/git" <<EOF
#!/usr/bin/env bash
if [[ "\$1" == "-C" ]]; then shift 2; fi
case "\$1 \$2" in
  "rev-parse --absolute-git-dir")
    if [[ "\$PWD" == "$CHILD" ]]; then
      printf '%s\n' "$PARENT/.git/worktrees/child"
    else
      printf '%s\n' "$PARENT/.git"
    fi
    ;;
  "rev-parse --path-format=absolute")
    printf '%s\n' "$PARENT/.git"
    ;;
  "worktree list")
    printf 'worktree %s\n' "$PARENT"
    ;;
  *) exit 0 ;;
esac
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/git"

  # Stub claude — must be on PATH but should NEVER actually run.
  cat >"$BATS_TEST_DIRNAME/fakes/claude" <<'EOF'
#!/usr/bin/env bash
echo 'fake claude was exec`d (test bug)' >&2
exit 99
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/claude"

  # Encoded project dir for the parent path. Matches the wrapper's
  # current encoder (s|/|-|g; s|_|-|g) for path components without
  # other special chars (TEST_HOME is /tmp/dux-amq-test.XXXX which is
  # all simple chars).
  ENC_PARENT=$(bash -c "source '$BATS_TEST_DIRNAME/../lib/path-encode.sh'; path_encode '$PARENT'")
  MAIN_PROJ_DIR="$HOME/.claude/projects/$ENC_PARENT"
  mkdir -p "$MAIN_PROJ_DIR"
  printf '{"role":"user"}\n' >"$MAIN_PROJ_DIR/aaaa.jsonl"
}

teardown() {
  rm -f "$BATS_TEST_DIRNAME/fakes/git" "$BATS_TEST_DIRNAME/fakes/claude"
  teardown_isolated_home
}

@test "wrapper is sourceable without exec'ing claude" {
  cd "$CHILD"
  run bash -c "source '$WRAPPER'; echo SOURCED_OK"
  [ "$status" -eq 0 ]
  [[ "$output" == *SOURCED_OK* ]]
  # If the source guard had not fired, the wrapper would have exec'd
  # claude (the fake) and exit 99 / printed the test-bug line.
  [[ "$output" != *"test bug"* ]]
}

@test "seeding off by default (no env var)" {
  cd "$CHILD"
  unset CLAUDE_AMQ_SEED_FROM_PARENT || true
  run bash -c "source '$WRAPPER'; seed_session_history; ls '$HOME/.claude/projects' 2>&1"
  [ "$status" -eq 0 ]
  ENC_CHILD=$(bash -c "source '$BATS_TEST_DIRNAME/../lib/path-encode.sh'; path_encode '$CHILD'")
  # Child's project dir must NOT have been created.
  [ ! -d "$HOME/.claude/projects/$ENC_CHILD" ]
}

@test "seeding fires when CLAUDE_AMQ_SEED_FROM_PARENT=1" {
  cd "$CHILD"
  export CLAUDE_AMQ_SEED_FROM_PARENT=1
  run bash -c "source '$WRAPPER'; seed_session_history"
  [ "$status" -eq 0 ]
  ENC_CHILD=$(bash -c "source '$BATS_TEST_DIRNAME/../lib/path-encode.sh'; path_encode '$CHILD'")
  [ -d "$HOME/.claude/projects/$ENC_CHILD" ]
  [ -f "$HOME/.claude/projects/$ENC_CHILD/aaaa.jsonl" ]
}

@test "no CLAUDE_AMQ_NO_SEED reference in wrapper or config code" {
  # README's "Migrating" bullet intentionally mentions the old var; only
  # check code/config locations.
  run grep -RIn 'CLAUDE_AMQ_NO_SEED' \
    "$BATS_TEST_DIRNAME/../wrappers" \
    "$BATS_TEST_DIRNAME/../config" \
    "$BATS_TEST_DIRNAME/../scripts" \
    "$BATS_TEST_DIRNAME/../install.sh"
  [ "$status" -ne 0 ]
}
