#!/usr/bin/env bats
#
# Phase 03 (audit01 P0-4): finalize-claude-migration.sh hardening.
#
# These tests use a redirected $HOME and a per-test STATE_ROOT so they
# never touch /data/state. The fakes/ pgrep stub is used to simulate
# `claude` running during the migration.

load 'lib/setup'

SCRIPT="${BATS_TEST_DIRNAME%/tests}/scripts/finalize-claude-migration.sh"

setup() {
  setup_isolated_home
  STATE_ROOT="$(mktemp -d -t dux-amq-state.XXXXXX)"
  export STATE_ROOT
  # The script hard-codes /data/state/...; we don't have permission to write
  # there in CI/dev. Instead we run the script with the relevant lines
  # rewritten via a wrapper that sed-substitutes `/data/state` → $STATE_ROOT.
  WRAPPED_SCRIPT="$TEST_HOME/finalize.sh"
  sed "s|/data/state|${STATE_ROOT//|/\\|}|g" "$SCRIPT" > "$WRAPPED_SCRIPT"
  chmod +x "$WRAPPED_SCRIPT"
  # Per-test lock path so concurrent test workers don't collide.
  export DUX_AMQ_FINALIZE_LOCK="$TEST_HOME/finalize.lock"
}

teardown() {
  if [[ -n "${STATE_ROOT:-}" && -d "$STATE_ROOT" ]]; then
    rm -rf "$STATE_ROOT"
  fi
  teardown_isolated_home
}

# --- pgrep fake helpers --------------------------------------------------

# Always-clean pgrep: returns 1 (no claude process), used as the default.
install_pgrep_clean() {
  cat >"$BATS_TEST_DIRNAME/fakes/pgrep" <<'EOF'
#!/usr/bin/env bash
exit 1
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/pgrep"
}

# pgrep that returns clean for the first $1 calls then dirty (claude found)
# for all subsequent calls. State is kept in a counter file under $TEST_HOME.
install_pgrep_dirty_after() {
  local clean_calls="$1"
  local counter="$TEST_HOME/.pgrep_calls"
  echo 0 >"$counter"
  cat >"$BATS_TEST_DIRNAME/fakes/pgrep" <<EOF
#!/usr/bin/env bash
counter="$counter"
n=\$(<"\$counter")
n=\$((n+1))
echo "\$n" >"\$counter"
if (( n <= $clean_calls )); then
  exit 1
fi
echo "12345 claude (fake)"
exit 0
EOF
  chmod +x "$BATS_TEST_DIRNAME/fakes/pgrep"
}

cleanup_pgrep_fake() {
  rm -f "$BATS_TEST_DIRNAME/fakes/pgrep"
}

# --- tests ---------------------------------------------------------------

@test "happy path: ~/.claude becomes a symlink to STATE_ROOT/claude" {
  install_pgrep_clean
  mkdir -p "$HOME/.claude"
  echo "hello" >"$HOME/.claude/sentinel"
  mkdir -p "$HOME/.agents"
  run "$WRAPPED_SCRIPT"
  cleanup_pgrep_fake
  [ "$status" -eq 0 ]
  [ -L "$HOME/.claude" ]
  [ "$(readlink "$HOME/.claude")" = "$STATE_ROOT/claude" ]
  [ -f "$STATE_ROOT/claude/sentinel" ]
  [ -L "$HOME/.agents" ]
}

@test "rsync without --force does NOT delete pre-existing dst content" {
  install_pgrep_clean
  mkdir -p "$HOME/.claude"
  echo "src-only" >"$HOME/.claude/from-src"
  mkdir -p "$STATE_ROOT/claude"
  echo "preserve-me" >"$STATE_ROOT/claude/dst-only"
  mkdir -p "$HOME/.agents"
  run "$WRAPPED_SCRIPT"
  cleanup_pgrep_fake
  [ "$status" -eq 0 ]
  # Pre-existing file in dst must survive (no --delete by default).
  [ -f "$STATE_ROOT/claude/dst-only" ]
  [ -f "$STATE_ROOT/claude/from-src" ]
}

@test "concurrent run: second invocation aborts with lock message" {
  install_pgrep_clean
  mkdir -p "$HOME/.claude" "$HOME/.agents"
  # Hold the lock from a background flock(1).
  ( flock -x 9; sleep 5 ) 9>"$DUX_AMQ_FINALIZE_LOCK" &
  HOLDER=$!
  # Give the holder time to grab the lock.
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if ! flock -n 9 -c true 9<"$DUX_AMQ_FINALIZE_LOCK" 2>/dev/null; then
      break
    fi
    sleep 0.05
  done
  run "$WRAPPED_SCRIPT"
  kill "$HOLDER" 2>/dev/null || true
  wait "$HOLDER" 2>/dev/null || true
  cleanup_pgrep_fake
  [ "$status" -ne 0 ]
  echo "$output" | grep -q "another finalize/claude run holds"
}

@test "claude appearing mid-flight: recheck_no_claude aborts" {
  # First call (top-level) clean; second call (just before rsync) dirty.
  install_pgrep_dirty_after 1
  mkdir -p "$HOME/.claude" "$HOME/.agents"
  echo "x" >"$HOME/.claude/file"
  run "$WRAPPED_SCRIPT"
  cleanup_pgrep_fake
  [ "$status" -ne 0 ]
  echo "$output" | grep -q "claude.*started during migration"
  # Source still intact since rsync was aborted before destructive swap.
  [ -d "$HOME/.claude" ]
  [ ! -L "$HOME/.claude" ]
}

@test "atomicity: SIGKILL during migration leaves ~/.claude as symlink OR dir, never absent without backup" {
  install_pgrep_clean
  mkdir -p "$HOME/.claude" "$HOME/.agents"
  for i in 1 2 3 4 5; do
    echo "$i" >"$HOME/.claude/file-$i"
  done

  # Run the script under timeout that kills it mid-way at varying points.
  # We try several short timeouts; whichever fires, the invariant must hold.
  local killed=0
  for t in 0.01 0.05 0.1 0.2; do
    # Fresh state for each iteration.
    rm -rf "$HOME/.claude" "$HOME/.agents" "$STATE_ROOT"
    mkdir -p "$HOME/.claude" "$HOME/.agents" "$STATE_ROOT"
    for i in 1 2 3 4 5; do
      echo "$i" >"$HOME/.claude/file-$i"
    done
    timeout --signal=KILL "${t}s" "$WRAPPED_SCRIPT" >/dev/null 2>&1 || killed=1
    # Invariant: at script-exit, ~/.claude is EITHER a symlink to dst OR a
    # directory (the original or the backup-renamed-back). It can briefly be
    # absent between the two rename(2) calls, in which case the .bak.<ts>
    # backup MUST exist and point at the user's data.
    if [[ ! -e "$HOME/.claude" && ! -L "$HOME/.claude" ]]; then
      ls -A "$HOME"/.claude.bak.* >/dev/null 2>&1 || {
        echo "INVARIANT VIOLATION at timeout=${t}s: ~/.claude absent and no backup" >&2
        return 1
      }
    fi
  done
  cleanup_pgrep_fake
  # At least one iteration must have actually been killed for the test to
  # be meaningful. (timeout 0.01s is very likely to fire.)
  [ "$killed" -eq 1 ]
}

@test "stale regular file at /data/state/.agents is replaced by symlink" {
  install_pgrep_clean
  mkdir -p "$HOME/.claude" "$HOME/.agents"
  # Plant a non-symlink at the bridge path; the old `-e` check would skip,
  # leaving the bridge broken. The new `-L` check should replace it.
  mkdir -p "$STATE_ROOT"
  echo "stale" >"$STATE_ROOT/.agents"
  run "$WRAPPED_SCRIPT"
  cleanup_pgrep_fake
  [ "$status" -eq 0 ]
  [ -L "$STATE_ROOT/.agents" ]
  [ "$(readlink "$STATE_ROOT/.agents")" = "$STATE_ROOT/agents" ]
}
