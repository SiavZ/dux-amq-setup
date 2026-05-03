#!/usr/bin/env bats
#
# audit02 phase 08 (P0-K, T2): HMAC envelope auth + replay protection
# for the dux-amq inter-pane message bus. Tests the three new scripts
# in `dux-amq/scripts/`:
#
#   amq-secret-init.sh   — generates the per-VM secret (mode 0600)
#   amq-send-signed      — wraps a body in a signed DUX1 envelope
#   amq-receive-verify   — validates the envelope on stdin and emits
#                          the clean body on stdout, dropping unsigned
#                          / replayed / MAC-mismatched messages
#
# These are unit-level tests against the scripts in isolation
# (`--print-only` bypasses the live AMQ queue). End-to-end coverage
# of the wrapper integration with `amq wake --inject-via` is gated on
# upstream AMQ adding stdin piping for `--inject-via`; see the
# upstream issue note in the commit body of audit02/08-amq-message-auth.
#
# Run with `make overlay-bats` (or `bats dux-amq/tests/amq-auth.bats`).

load 'lib/setup'

SCRIPTS_DIR="$BATS_TEST_DIRNAME/../scripts"

setup() {
  setup_isolated_home
  # Prepend $SCRIPTS_DIR to PATH so amq-receive-verify (which lives in
  # the same dir) and amq-send-signed are discoverable without absolute
  # paths in the test bodies.
  export PATH="$SCRIPTS_DIR:$PATH"
  # Each test gets a fresh $HOME, so the secret is regenerated and the
  # nonce dedup file (XDG_RUNTIME_DIR or /tmp/dux-amq) starts empty.
  export XDG_RUNTIME_DIR="$TEST_HOME/run"
  mkdir -p "$XDG_RUNTIME_DIR"
  # Seed the secret unconditionally — every test needs it.
  "$SCRIPTS_DIR/amq-secret-init.sh" >/dev/null 2>&1
}

teardown() {
  teardown_isolated_home
}

# 8.1 prerequisite: the secret file is written with mode 0600.
@test "amq-secret-init.sh writes 32-byte base64 secret with mode 0600" {
  local secret="$HOME/.local/share/dux-amq/amq-secret"
  [[ -f "$secret" ]]
  # 256 bits → 44 base64 chars (with `=` padding).
  local n
  n=$(wc -c <"$secret")
  [[ "$n" -ge 43 && "$n" -le 46 ]]
  local mode
  mode=$(stat -c '%a' "$secret" 2>/dev/null || stat -f '%Lp' "$secret")
  [[ "$mode" == "600" ]]
}

# Test 1 (plan §8.6): unsigned messages are dropped with a stderr warning.
@test "amq-receive-verify drops unsigned messages" {
  run bash -c 'echo "plain text not signed" | amq-receive-verify'
  [[ "$status" -eq 0 ]]
  # Body must NOT be re-emitted on stdout — that would defeat the filter.
  [[ -z "$output" || "$output" =~ ^\[amq-verify\] ]]
  # Stderr (captured into $output by bats run when stdout is empty) must
  # mention the drop reason. Re-run capturing stderr explicitly to assert.
  run bash -c 'echo "plain text not signed" | amq-receive-verify 2>&1 1>/dev/null'
  [[ "$output" == *"dropping unsigned"* ]]
}

# Test 2 (plan §8.6): a well-signed envelope round-trips its body.
@test "amq-receive-verify accepts a well-signed message and emits the body" {
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "hello" --print-only)
  run bash -c "printf '%s\n' \"$msg\" | amq-receive-verify"
  [[ "$status" -eq 0 ]]
  [[ "$output" == "hello" ]]
}

# Test 3 (plan §8.6): replaying the same envelope is rejected.
@test "amq-receive-verify rejects replayed envelopes (nonce dedup)" {
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "hi" --print-only)
  # First delivery: accepted.
  run bash -c "printf '%s\n' \"$msg\" | amq-receive-verify"
  [[ "$status" -eq 0 ]]
  [[ "$output" == "hi" ]]
  # Second delivery: dropped on nonce match.
  run bash -c "printf '%s\n' \"$msg\" | amq-receive-verify 2>&1 1>/dev/null"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"replay rejected"* ]]
  # And the body must NOT appear on stdout (drop = stderr only).
  run bash -c "printf '%s\n' \"$msg\" | amq-receive-verify 2>/dev/null"
  [[ -z "$output" ]]
}

# Test 4 (plan §8.6): tampering the body invalidates the MAC.
@test "amq-receive-verify rejects MAC-mismatched envelopes" {
  local msg bad
  msg=$(amq-send-signed --me alice --to bob --body "hi" --print-only)
  # Replace the body field with a different value, leaving the MAC intact.
  bad=${msg/hi/EVIL}
  run bash -c "printf '%s\n' \"$bad\" | amq-receive-verify 2>&1 1>/dev/null"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"HMAC mismatch"* ]]
  # And critically: the tampered body must NOT appear on stdout.
  run bash -c "printf '%s\n' \"$bad\" | amq-receive-verify 2>/dev/null"
  [[ -z "$output" ]]
}

# Audit02 Phase 13: argv-mode input fallback. AMQ v0.34.0's
# `--inject-via` invokes the executable with the envelope as the FINAL
# argv element (no stdin). Without the argv path, the verifier would
# silently no-op for every wake notification under v0.34.0.

@test "amq-receive-verify accepts envelope passed as argv \$1 (Phase 13)" {
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "via-argv" --print-only)
  # Close stdin (`</dev/null`) so the script must take the argv branch.
  run bash -c "amq-receive-verify \"$msg\" </dev/null"
  [[ "$status" -eq 0 ]]
  [[ "$output" == "via-argv" ]]
}

@test "amq-receive-verify drops unsigned argv-mode envelope" {
  run bash -c 'amq-receive-verify "plain-not-signed" </dev/null 2>&1 1>/dev/null'
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"dropping unsigned"* ]]
  # And no body on stdout.
  run bash -c 'amq-receive-verify "plain-not-signed" </dev/null 2>/dev/null'
  [[ -z "$output" ]]
}

@test "amq-receive-verify argv-mode replay rejection (nonce dedup)" {
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "argv-dup" --print-only)
  # First delivery via argv: accepted.
  run bash -c "amq-receive-verify \"$msg\" </dev/null"
  [[ "$status" -eq 0 ]]
  [[ "$output" == "argv-dup" ]]
  # Second delivery via argv: dropped on nonce match.
  run bash -c "amq-receive-verify \"$msg\" </dev/null 2>&1 1>/dev/null"
  [[ "$status" -eq 0 ]]
  [[ "$output" == *"replay rejected"* ]]
}

@test "amq-receive-verify prefers stdin over argv when both present" {
  local stdin_msg argv_msg
  stdin_msg=$(amq-send-signed --me alice --to bob --body "from-stdin" --print-only)
  argv_msg=$(amq-send-signed --me alice --to bob --body "from-argv" --print-only)
  # When stdin has a valid envelope, argv must be ignored.
  run bash -c "printf '%s\n' \"$stdin_msg\" | amq-receive-verify \"$argv_msg\""
  [[ "$status" -eq 0 ]]
  [[ "$output" == "from-stdin" ]]
}

@test "amq-receive-verify falls back to argv when stdin is empty" {
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "stdin-empty-argv-wins" --print-only)
  # `:` is the no-op command — its output is empty. The script's
  # IFS= read will return non-zero and the argv branch must take over.
  run bash -c "amq-receive-verify \"$msg\" < <(:)"
  [[ "$status" -eq 0 ]]
  [[ "$output" == "stdin-empty-argv-wins" ]]
}
