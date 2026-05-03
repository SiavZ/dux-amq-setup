#!/usr/bin/env bats
#
# Phase 08: wake-daemon startup probe.
#
# Cases:
#   - Failure path: fake `amq` exits 1 immediately. wake_launch returns
#     non-zero, the per-pane log file exists and has the failure output.
#   - Success path: fake `amq wake` sleeps for several seconds.
#     wake_launch returns 0 and `kill -0 $!` succeeds during the probe.
#   - Log rotation: a >5 MiB existing log gets rotated to .1 on next
#     launch (smoke test for the rotation branch).
#   - Static check: no `>/dev/null 2>&1` on `amq wake` anywhere in the
#     three wrappers.

load 'lib/setup'

LIB="$BATS_TEST_DIRNAME/../lib/wake-launch.sh"

setup() {
  setup_isolated_home
}

teardown() {
  rm -f "$BATS_TEST_DIRNAME/fakes/amq"
  teardown_isolated_home
}

@test "failure path: amq exits 1, wake_launch returns non-zero, log captures stderr" {
  cat >"$BATS_TEST_DIRNAME/fakes/amq" <<'EOF'
#!/usr/bin/env bash
echo "boom: TIOCSTI denied" >&2
exit 1
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/amq"

  # Probe time short enough to keep the test snappy; long enough that
  # the fake's exit completes before kill -0.
  run env DUX_AMQ_WAKE_STDIN=/dev/null DUX_AMQ_WAKE_PROBE_SECS=0.3 bash -c "
    set +e
    source '$LIB'
    wake_launch testpeer /tmp 2>&1
    echo \"rc=\$?\"
  "
  [[ "$output" == *"rc=1"* ]]
  [[ "$output" == *"amq wake failed"* ]]
  [ -f "$HOME/.local/share/dux-amq/wake-testpeer.log" ]
  grep -q "boom: TIOCSTI denied" "$HOME/.local/share/dux-amq/wake-testpeer.log"
}

@test "success path: amq sleeps long enough, wake_launch returns 0" {
  cat >"$BATS_TEST_DIRNAME/fakes/amq" <<'EOF'
#!/usr/bin/env bash
sleep 10
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/amq"

  run env DUX_AMQ_WAKE_STDIN=/dev/null DUX_AMQ_WAKE_PROBE_SECS=0.2 bash -c "
    set +e
    source '$LIB'
    wake_launch alivepeer /tmp
    rc=\$?
    echo \"rc=\$rc\"
  "
  [[ "$output" == *"rc=0"* ]]
  # Ensure no failure banner.
  [[ "$output" != *"amq wake failed"* ]]
  # Cleanup: the fake amq is still sleeping in the background; nuke it.
  pkill -f 'sleep 10' 2>/dev/null || true
}

@test "DUX_AMQ_WAKE_STRICT=1 changes nothing on success" {
  cat >"$BATS_TEST_DIRNAME/fakes/amq" <<'EOF'
#!/usr/bin/env bash
sleep 10
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/amq"

  run env DUX_AMQ_WAKE_STDIN=/dev/null DUX_AMQ_WAKE_PROBE_SECS=0.2 DUX_AMQ_WAKE_STRICT=1 bash -c "
    set +e
    source '$LIB'
    wake_launch strictpeer /tmp
    echo \"rc=\$?\"
  "
  [[ "$output" == *"rc=0"* ]]
  pkill -f 'sleep 10' 2>/dev/null || true
}

@test "log rotation: >5 MiB existing log → moved to .1" {
  mkdir -p "$HOME/.local/share/dux-amq"
  # Create a 5 MiB + 1 byte file fast.
  dd if=/dev/zero of="$HOME/.local/share/dux-amq/wake-rotpeer.log" \
     bs=1M count=5 status=none
  printf 'one-extra-byte' >> "$HOME/.local/share/dux-amq/wake-rotpeer.log"
  # Failing fake so the test finishes quickly.
  cat >"$BATS_TEST_DIRNAME/fakes/amq" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/amq"

  run env DUX_AMQ_WAKE_STDIN=/dev/null DUX_AMQ_WAKE_PROBE_SECS=0.3 bash -c "
    set +e
    source '$LIB'
    wake_launch rotpeer /tmp >/dev/null 2>&1
    exit 0
  "
  [ -f "$HOME/.local/share/dux-amq/wake-rotpeer.log.1" ]
}

@test "no '>/dev/null 2>&1' on amq wake anywhere in wrappers" {
  for w in "$BATS_TEST_DIRNAME/../wrappers/"{claude,codex,gemini}-amq; do
    run grep -E 'amq wake.*>/dev/null 2>&1' "$w"
    [ "$status" -ne 0 ]
  done
}

@test "wrappers source wake-launch.sh and call wake_launch" {
  for w in "$BATS_TEST_DIRNAME/../wrappers/"{claude,codex,gemini}-amq; do
    grep -q 'wake-launch.sh' "$w"
    grep -q 'wake_launch ' "$w"
  done
}
