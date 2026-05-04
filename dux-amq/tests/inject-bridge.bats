#!/usr/bin/env bats
#
# audit02 Phase 13 (audit01 P1-1): TIOCSTI fallback bridge.
#
# Validates that `dux-amq-inject-bridge`:
#   * Verifies the envelope through `amq-receive-verify` BEFORE
#     attempting any injection.
#   * Drops unsigned / replayed / MAC-mismatched messages silently
#     (exit 0, no body emitted, nothing typed into the TTY).
#   * Uses `tmux send-keys` when $TMUX is set, `tmux` is on PATH, and
#     dux is NOT the parent process tree (no $DUX_PANE).
#   * When $DUX_PANE is set (running under dux), ALWAYS writes to the
#     file queue regardless of $TMUX, so the dux-side drainer can
#     deliver the body once the agent is idle. This avoids the
#     "stuck in input field" failure where Claude Code's Ink input
#     drops a trailing Enter received during streaming.
#   * Routes queue files to a per-receiver subdirectory keyed off
#     $AM_ME (sanitised), with `_unrouted/` as the fallback when
#     $AM_ME is missing.
#
# `tmux` is shimmed via tests/fakes/ so tests can record what the
# bridge would have sent without needing a real tmux server.

load 'lib/setup'

SCRIPTS_DIR="$BATS_TEST_DIRNAME/../scripts"

setup() {
  setup_isolated_home
  # Ensure both the bridge and verify are reachable. setup_isolated_home
  # already prepends $BATS_TEST_DIRNAME/../scripts to PATH.
  export XDG_RUNTIME_DIR="$TEST_HOME/run"
  mkdir -p "$XDG_RUNTIME_DIR"
  # Seed the per-VM HMAC secret. amq-secret-init.sh is idempotent.
  "$SCRIPTS_DIR/amq-secret-init.sh" >/dev/null 2>&1

  # Per-test fake tmux that records its argv. The fake replaces tmux
  # *only* when we explicitly install it on PATH inside a test;
  # default state has no tmux on PATH so the file-queue branch
  # exercises naturally.
  TMUX_LOG="$TEST_HOME/tmux.log"
  : >"$TMUX_LOG"
  export TMUX_LOG

  # Force-unset $TMUX and $DUX_PANE so the default test environment
  # doesn't accidentally trip a non-default branch via the parent
  # shell's session.
  unset TMUX
  unset DUX_PANE
  unset AM_ME
}

teardown() {
  teardown_isolated_home
}

# Helper: install a fake `tmux` on PATH that logs its argv to $TMUX_LOG.
install_fake_tmux() {
  local fake_dir="$TEST_HOME/bin"
  mkdir -p "$fake_dir"
  cat >"$fake_dir/tmux" <<'EOF'
#!/usr/bin/env bash
# Fake tmux for inject-bridge tests. Records every invocation as
# "ARGV\n<arg>\n...END\n" so tests can grep the log.
{
  printf 'ARGV\n'
  for a in "$@"; do
    printf '%s\n' "$a"
  done
  printf 'END\n'
} >>"$TMUX_LOG"
EOF
  chmod 0755 "$fake_dir/tmux"
  PATH="$fake_dir:$PATH"
  export PATH
}

# 13.1 — happy path with tmux (no DUX_PANE): verified body → send-keys.
@test "dux-amq-inject-bridge sends verified body via tmux send-keys when not under dux" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "hello-world" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # tmux must have been called, with the body and a literal Enter.
  grep -Fxq -- "send-keys" "$TMUX_LOG"
  grep -Fxq -- "hello-world" "$TMUX_LOG"
  grep -Fxq -- "Enter" "$TMUX_LOG"
  # No file in any subdirectory of the queue when tmux delivery succeeded.
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null
}

# 13.2 — DUX_TMUX_TARGET is honored.
@test "dux-amq-inject-bridge uses DUX_TMUX_TARGET when set" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export DUX_TMUX_TARGET="mywin:0.1"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "targeted" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  grep -Fxq -- "-t" "$TMUX_LOG"
  grep -Fxq -- "mywin:0.1" "$TMUX_LOG"
}

# 13.3 — security: unsigned envelopes are NEVER injected.
@test "dux-amq-inject-bridge drops unsigned envelope without injecting" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  run dux-amq-inject-bridge "plain-spoofed-text"
  [ "$status" -eq 0 ]
  # tmux must NOT have been called at all.
  [ ! -s "$TMUX_LOG" ]
  # File queue must be empty (no per-receiver subdir created).
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null
}

# 13.4 — security: MAC-mismatched envelopes are NEVER injected.
@test "dux-amq-inject-bridge drops MAC-mismatched envelope" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  local msg bad
  msg=$(amq-send-signed --me alice --to bob --body "real" --print-only)
  bad=${msg/real/EVIL}
  run dux-amq-inject-bridge "$bad"
  [ "$status" -eq 0 ]
  [ ! -s "$TMUX_LOG" ]
}

# 13.5 — fallback (no TMUX, no AM_ME): body lands in `_unrouted/`.
@test "dux-amq-inject-bridge falls back to _unrouted queue without TMUX or AM_ME" {
  unset TMUX
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "queued-msg" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # Exactly one file under `_unrouted/`, containing the body.
  local files
  mapfile -t files < <(compgen -G "$HOME/.local/share/dux-amq/inject-queue/_unrouted/*.msg")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "queued-msg" "${files[0]}"
  # No file at the legacy flat path (no top-level *.msg).
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*.msg" >/dev/null
}

# 13.6 — empty argv: bridge must exit 0 silently (verify dropped it).
@test "dux-amq-inject-bridge handles empty argv silently" {
  unset TMUX
  run dux-amq-inject-bridge ""
  [ "$status" -eq 0 ]
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null
}

# 13.7 — under dux: DUX_PANE=1 forces the queue path even with TMUX.
@test "dux-amq-inject-bridge prefers queue over tmux when DUX_PANE is set" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export DUX_PANE="1"
  export AM_ME="bob"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "under-dux" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # tmux must NOT have been called — DUX_PANE wins over $TMUX.
  [ ! -s "$TMUX_LOG" ]
  # Body must be queued under bob/, not _unrouted/.
  local files
  mapfile -t files < <(compgen -G "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "under-dux" "${files[0]}"
}

# 13.8 — receiver path: AM_ME determines the subdirectory.
@test "dux-amq-inject-bridge keys queue files on sanitised AM_ME" {
  unset TMUX
  export AM_ME="bob"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "addressed" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  local files
  mapfile -t files < <(compgen -G "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "addressed" "${files[0]}"
}

# 13.9 — receiver sanitisation: uppercase + bad chars normalised to
# the same lowercase regex the wrappers apply, blocking path traversal.
@test "dux-amq-inject-bridge sanitises AM_ME (uppercase, slashes, dots)" {
  unset TMUX
  # Mirrors the wrapper sanitisation: tr '[:upper:]' '[:lower:]' then
  # sed 's|[^a-z0-9_-]|-|g; s|^-\+||; s|-\+$||'. So "Feature/Login.v2"
  # becomes "feature-login-v2".
  export AM_ME="Feature/Login.v2"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "sanitised" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  local files
  mapfile -t files < <(compgen -G "$HOME/.local/share/dux-amq/inject-queue/feature-login-v2/*.msg")
  [ "${#files[@]}" -eq 1 ]
  # The unsanitised value MUST NOT exist as a directory — defence
  # against `..` or absolute paths sneaking in through AM_ME.
  [ ! -d "$HOME/.local/share/dux-amq/inject-queue/Feature/Login.v2" ]
}

# 13.10 — receiver sanitisation cannot escape the queue root.
@test "dux-amq-inject-bridge rejects path-traversal AM_ME" {
  unset TMUX
  # `..` would map to `--` after sed, which then has leading dashes
  # stripped to empty. Empty receivers fall back to `_unrouted/`.
  export AM_ME="../../etc"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "traversal" --print-only)
  run dux-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # The body landed somewhere INSIDE inject-queue/, not above it.
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/../*.msg" >/dev/null
  # And not in any directory derived from the literal `../../etc`.
  [ ! -e "$HOME/.local/share/etc" ]
}
