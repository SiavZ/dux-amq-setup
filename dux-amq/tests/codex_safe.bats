#!/usr/bin/env bats
#
# Phase 05: Codex YOLO opt-out + threat-model docs.
#
# - Default (CODEX_AMQ_SAFE unset): wrapper builds CODEX_EXTRA containing
#   `--dangerously-bypass-approvals-and-sandbox`.
# - CODEX_AMQ_SAFE=1: wrapper builds CODEX_EXTRA empty.
# - Static checks: claude default unchanged (still default-on YOLO), README
#   carries a "Security model" section, bashrc-additions mentions both
#   opt-outs.

load 'lib/setup'

CODEX_WRAPPER="$BATS_TEST_DIRNAME/../wrappers/codex-amq"
CLAUDE_WRAPPER="$BATS_TEST_DIRNAME/../wrappers/claude-amq"
README="$BATS_TEST_DIRNAME/../README.md"
BASHRC="$BATS_TEST_DIRNAME/../config/bashrc-additions.sh"

setup() {
  setup_isolated_home
  cat >"$BATS_TEST_DIRNAME/fakes/codex" <<'EOF'
#!/usr/bin/env bash
echo 'fake codex was exec`d (test bug)' >&2
exit 99
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/codex"
}

teardown() {
  rm -f "$BATS_TEST_DIRNAME/fakes/codex"
  teardown_isolated_home
}

@test "default: CODEX_EXTRA contains the bypass flag" {
  unset CODEX_AMQ_SAFE || true
  # Source the wrapper. Its `(return 0 …) && return 0` guard sits AFTER
  # CODEX_EXTRA is built, so we can inspect the array.
  run bash -c "
    set +u
    AM_ME=test ROOT=/tmp source '$CODEX_WRAPPER'
    printf '%s\n' \"\${CODEX_EXTRA[@]}\"
  "
  [ "$status" -eq 0 ]
  [[ "$output" == *"--dangerously-bypass-approvals-and-sandbox"* ]]
}

@test "CODEX_AMQ_SAFE=1: CODEX_EXTRA is empty (no bypass flag)" {
  export CODEX_AMQ_SAFE=1
  run bash -c "
    set +u
    AM_ME=test ROOT=/tmp source '$CODEX_WRAPPER'
    if [[ \${#CODEX_EXTRA[@]} -eq 0 ]]; then
      echo NO_FLAGS
    else
      printf 'UNEXPECTED: %s\n' \"\${CODEX_EXTRA[@]}\"
    fi
  "
  [ "$status" -eq 0 ]
  [[ "$output" == *NO_FLAGS* ]]
  [[ "$output" != *"--dangerously-bypass"* ]]
}

@test "Claude default permission flag is UNCHANGED (still default-on)" {
  # User mandate: do NOT flip the Claude default. The CLAUDE_AMQ_SAFE
  # opt-out must remain default-OFF (i.e. flag is added unless var is 1).
  grep -qE '\[\[ "\$\{CLAUDE_AMQ_SAFE:-\}" != "1" \]\]' "$CLAUDE_WRAPPER"
  grep -q -- '--dangerously-skip-permissions' "$CLAUDE_WRAPPER"
}

@test "Codex wrapper has CODEX_AMQ_SAFE opt-out (symmetric with claude)" {
  grep -qE '\[\[ "\$\{CODEX_AMQ_SAFE:-\}" != "1" \]\]' "$CODEX_WRAPPER"
  grep -q -- '--dangerously-bypass-approvals-and-sandbox' "$CODEX_WRAPPER"
}

@test "README has a Security model section covering threat model + LUKS" {
  grep -q '^## Security model' "$README"
  grep -q -i 'threat model' "$README"
  grep -q 'cryptsetup luksFormat' "$README"
  grep -q 'CLAUDE_AMQ_SAFE' "$README"
  grep -q 'CODEX_AMQ_SAFE' "$README"
  grep -q -i 'auto mode' "$README"
}

@test "bashrc-additions.sh mentions both opt-outs" {
  grep -q 'CLAUDE_AMQ_SAFE' "$BASHRC"
  grep -q 'CODEX_AMQ_SAFE' "$BASHRC"
}
