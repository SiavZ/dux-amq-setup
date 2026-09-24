#!/usr/bin/env bats
# Phase 2: shared-root ownership markers and mandatory registration lock.

load 'lib/setup'

WRAPPERS_DIR="$BATS_TEST_DIRNAME/../wrappers"

setup() {
  setup_isolated_home
  export STATE_ROOT="$TEST_HOME/state"
  export AMQ_GLOBAL_ROOT="$TEST_HOME/amq"
  export DUX_AMQ_INJECT_MODE=via
  export AMQ_FAKE_ARGV_FILE="$TEST_HOME/argv.log"
  mkdir -p "$STATE_ROOT/dux" "$AMQ_GLOBAL_ROOT/agents"
  : >"$AMQ_FAKE_ARGV_FILE"
  unset AM_ME DUX_STORE_ID DUX_SESSION_ID DUX_AMQ_HANDLE DUX_AMQ_FLOCK
}

teardown() {
  teardown_isolated_home
}

@test "managed wrappers atomically record store and session without a detached wake PID" {
  local provider wrapper handle session marker mode
  for provider in claude codex gemini; do
    wrapper="$WRAPPERS_DIR/$provider-amq"
    handle="$provider-managed"
    session="$provider-session"
    DUX_STORE_ID=store-a DUX_SESSION_ID="$session" DUX_AMQ_HANDLE="$handle" run "$wrapper"
    [ "$status" -eq 0 ]
    marker="$AMQ_GLOBAL_ROOT/agents/$handle/.dux-amq-source"
    jq -e --arg session "$session" \
      '.store_id == "store-a" and .session_id == $session and (has("wake_pid") | not)' \
      "$marker" >/dev/null
    mode=$(stat -c '%a' "${marker%/*}" 2>/dev/null || stat -f '%Lp' "${marker%/*}")
    [ "$mode" = "700" ]
  done
}

@test "managed handle wins over inherited AM_ME" {
  AM_ME=wrong DUX_STORE_ID=store-a DUX_SESSION_ID=session-a DUX_AMQ_HANDLE=right \
    run "$WRAPPERS_DIR/codex-amq"

  [ "$status" -eq 0 ]
  [ -f "$AMQ_GLOBAL_ROOT/agents/right/.dux-amq-source" ]
  [ ! -e "$AMQ_GLOBAL_ROOT/agents/wrong" ]
}

@test "wrapper fails closed when mandatory flock is unavailable" {
  DUX_AMQ_FLOCK=definitely-not-a-flock-command run "$WRAPPERS_DIR/claude-amq"

  [ "$status" -ne 0 ]
  [[ "$output" == *"mandatory flock utility is unavailable"* ]]
  [ ! -e "$AMQ_GLOBAL_ROOT/agents/testpane" ]
}

@test "partial managed ownership environment fails closed" {
  DUX_STORE_ID=store-a DUX_SESSION_ID=session-a run "$WRAPPERS_DIR/gemini-amq"

  [ "$status" -ne 0 ]
  [[ "$output" == *"must be set together"* ]]
}

@test "managed handle is validated verbatim instead of repaired" {
  DUX_STORE_ID=store-a DUX_SESSION_ID=session-a DUX_AMQ_HANDLE='Bad/Handle' \
    run "$WRAPPERS_DIR/codex-amq"

  [ "$status" -ne 0 ]
  [[ "$output" == *"invalid or overlong AMQ handle"* ]]
  [ ! -e "$AMQ_GLOBAL_ROOT/agents/bad-handle" ]
}

@test "foreign JSON owner is preserved and managed launch is refused" {
  local marker="$AMQ_GLOBAL_ROOT/agents/taken/.dux-amq-source"
  mkdir -p "${marker%/*}"
  printf '{"store_id":"store-b","session_id":"foreign"}\n' >"$marker"

  DUX_STORE_ID=store-a DUX_SESSION_ID=local DUX_AMQ_HANDLE=taken \
    run "$WRAPPERS_DIR/claude-amq"

  [ "$status" -ne 0 ]
  jq -e '.store_id == "store-b" and .session_id == "foreign"' "$marker" >/dev/null
}

@test "concurrent managed wrappers leave valid config and exact owners" {
  local provider pid pids=()
  for provider in claude codex gemini; do
    (
      DUX_STORE_ID=store-a DUX_SESSION_ID="$provider-session" DUX_AMQ_HANDLE="$provider-agent" \
        "$WRAPPERS_DIR/$provider-amq"
    ) &
    pids+=("$!")
  done
  for pid in "${pids[@]}"; do
    wait "$pid"
  done

  jq -e '.agents == ["claude-agent","codex-agent","gemini-agent"]' \
    "$AMQ_GLOBAL_ROOT/meta/config.json" >/dev/null
  for provider in claude codex gemini; do
    jq -e --arg provider "$provider" \
      '.store_id == "store-a" and .session_id == ($provider + "-session")' \
      "$AMQ_GLOBAL_ROOT/agents/$provider-agent/.dux-amq-source" >/dev/null
  done
}
