#!/usr/bin/env bats
#
# audit02 Phase 13 (audit01 P1-1): TIOCSTI fallback bridge.
#
# Validates that `yaran-amq-inject-bridge`:
#   * Defaults to SKIP mode: transparently unwraps a `YARAN2\t...`
#     envelope when present (so signed senders interop), and delivers
#     plain bodies as-is otherwise. No HMAC check. This matches the
#     trust model in SECURITY.md: same-UID peers share $HOME so an
#     HMAC secret in $HOME isn't a defensible boundary, and the
#     verifier was silently dropping every legacy `amq send` message.
#   * In STRICT mode (YARAN_AMQ_VERIFY=1, opt-in): runs the envelope
#     through `amq-receive-verify` and drops on any verification
#     failure (unsigned/replayed/stale/MAC-mismatch). Reserved for
#     environments that genuinely cross a trust boundary.
#   * Uses `tmux send-keys` when $TMUX is set, `tmux` is on PATH, and
#     yaran is NOT the parent process tree (no $YARAN_PANE).
#   * When $YARAN_PANE is set (running under yaran), ALWAYS writes to the
#     file queue regardless of $TMUX, so the yaran-side drainer can
#     deliver the body once the agent is idle. This avoids the
#     "stuck in input field" failure where Claude Code's Ink input
#     drops a trailing Enter received during streaming.
#   * Routes queue files to a per-receiver subdirectory keyed off
#     $AM_ME (sanitised), with `.unrouted/` as the fallback when
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

  # Force-unset $TMUX and $YARAN_PANE so the default test environment
  # doesn't accidentally trip a non-default branch via the parent
  # shell's session.
  unset TMUX
  unset YARAN_PANE
  unset AM_ME
  unset AM_ROOT
  unset AMQ_GLOBAL_ROOT
  unset YARAN_AMQ_VERIFY
  unset YARAN_AMQ_STARTUP_OWNER_PID
  unset YARAN_PID
  unset AMQ_DRAIN_EMPTY
}

teardown() {
  teardown_isolated_home
}

@test "bridge honors canonical queue override instead of splitting the inbox" {
  export AM_ME=bob YARAN_PANE=1
  YARAN_AMQ_QUEUE_DIR="$TEST_HOME/current" DUX_AMQ_QUEUE_DIR="$TEST_HOME/old" run yaran-amq-inject-bridge "hello"
  [ "$status" -eq 0 ]
  collect_queue_files "$TEST_HOME/current/bob/*.msg"
  [ "${#QUEUE_FILES[@]}" -eq 1 ]
  [ ! -e "$TEST_HOME/old" ]
}

@test "bridge accepts legacy queue override" {
  export AM_ME=bob YARAN_PANE=1
  DUX_AMQ_QUEUE_DIR="$TEST_HOME/old" run yaran-amq-inject-bridge "hello"
  [ "$status" -eq 0 ]
  collect_queue_files "$TEST_HOME/old/bob/*.msg"
  [ "${#QUEUE_FILES[@]}" -eq 1 ]
}

@test "empty canonical queue override disables legacy delivery" {
  export AM_ME=bob YARAN_PANE=1
  YARAN_AMQ_QUEUE_DIR= DUX_AMQ_QUEUE_DIR="$TEST_HOME/old" run yaran-amq-inject-bridge "hello"
  [ "$status" -eq 0 ]
  [[ "$output" == *"queue delivery disabled"* ]]
  [ ! -e "$TEST_HOME/old" ]
}

@test "bridge XDG default remains in the shared dux-amq namespace" {
  export AM_ME=bob YARAN_PANE=1 XDG_DATA_HOME="$TEST_HOME/data"
  run yaran-amq-inject-bridge "hello"
  [ "$status" -eq 0 ]
  collect_queue_files "$XDG_DATA_HOME/dux-amq/inject-queue/bob/*.msg"
  [ "${#QUEUE_FILES[@]}" -eq 1 ]
  [ ! -e "$XDG_DATA_HOME/yaran-amq" ]
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

install_fake_drain_amq() {
  local fake_dir="$TEST_HOME/bin"
  mkdir -p "$fake_dir"
  cat >"$fake_dir/amq" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == "drain" ]]; then
  {
    printf 'ARGV\n'
    for a in "$@"; do
      printf '%s\n' "$a"
    done
    printf 'END\n'
  } >>"${AMQ_DRAIN_ARGV_LOG:?}"
  if [[ "${AMQ_DRAIN_EMPTY:-0}" == "1" ]]; then
    printf 'note: pending in a sibling session\n' >&2
    printf '{"drained":[],"count":0}\n'
  else
    printf '{"drained":[{"id":"message-1","from":"alice","to":["bob"],"thread":"p2p/alice__bob","subject":"hello","body":"drained-body\\u0003\\n","moved_to_cur":true}],"count":1}\n'
  fi
  exit 0
fi
exit 99
EOF
  chmod 0755 "$fake_dir/amq"
  PATH="$fake_dir:$PATH"
  export PATH
}

collect_queue_files() {
  QUEUE_FILES=()
  local file
  while IFS= read -r file; do
    [[ -n "$file" ]] && QUEUE_FILES+=("$file")
  done < <(compgen -G "$1" || true)
}

# 13.1 — happy path with tmux (no YARAN_PANE): verified body → send-keys.
@test "yaran-amq-inject-bridge sends verified body via tmux send-keys when not under yaran" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "hello-world" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # tmux must have been called, with the body and a literal Enter.
  grep -Fxq -- "send-keys" "$TMUX_LOG"
  grep -Fxq -- "hello-world" "$TMUX_LOG"
  grep -Fxq -- "Enter" "$TMUX_LOG"
  # No file in any subdirectory of the queue when tmux delivery succeeded.
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null || false
}

# 13.2 — YARAN_TMUX_TARGET is honored.
@test "yaran-amq-inject-bridge uses YARAN_TMUX_TARGET when set" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export YARAN_TMUX_TARGET="mywin:0.1"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "targeted" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  grep -Fxq -- "-t" "$TMUX_LOG"
  grep -Fxq -- "mywin:0.1" "$TMUX_LOG"
}

# 13.3 — strict mode: unsigned envelopes are dropped silently.
@test "yaran-amq-inject-bridge (strict) drops unsigned envelope without injecting" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export YARAN_AMQ_VERIFY=1
  export AM_ME="bob"
  run yaran-amq-inject-bridge "plain-spoofed-text"
  [ "$status" -eq 0 ]
  # tmux must NOT have been called at all.
  [ ! -s "$TMUX_LOG" ]
  # File queue must be empty (no per-receiver subdir created).
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null || false
}

# 13.4 — strict mode: MAC-mismatched envelopes are dropped.
@test "yaran-amq-inject-bridge (strict) drops MAC-mismatched envelope" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export YARAN_AMQ_VERIFY=1
  export AM_ME="bob"
  local msg bad
  msg=$(amq-send-signed --me alice --to bob --body "real" --print-only)
  local -a fields
  IFS=$'\t' read -ra fields <<<"$msg"
  fields[5]="RVZJTA=="
  bad=$(printf '%s\t%s\t%s\t%s\t%s\t%s\t%s' \
    "${fields[0]}" "${fields[1]}" "${fields[2]}" "${fields[3]}" \
    "${fields[4]}" "${fields[5]}" "${fields[6]}")
  run yaran-amq-inject-bridge "$bad"
  [ "$status" -eq 0 ]
  [ ! -s "$TMUX_LOG" ]
}

# 13.5 — fallback (no TMUX, no AM_ME): body lands in `.unrouted/`.
@test "yaran-amq-inject-bridge falls back to .unrouted queue without TMUX or AM_ME" {
  unset TMUX
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "queued-msg" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # Exactly one file under `.unrouted/`, containing the body.
  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/.unrouted/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "queued-msg" "${files[0]}"
  # No file at the legacy flat path (no top-level *.msg).
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*.msg" >/dev/null || false
}

@test "_unrouted remains a legitimate addressed receiver" {
  unset TMUX
  export AM_ME="_unrouted"
  local msg
  msg=$(amq-send-signed --me alice --to _unrouted --body "addressed" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/_unrouted/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "addressed" "${files[0]}"
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/.unrouted/*.msg" >/dev/null || false
}

# 13.6 — empty argv: bridge must exit 0 silently (verify dropped it).
@test "yaran-amq-inject-bridge handles empty argv silently" {
  unset TMUX
  run yaran-amq-inject-bridge ""
  [ "$status" -eq 0 ]
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null || false
}

# 13.7 — under yaran: YARAN_PANE=1 forces the queue path even with TMUX.
@test "yaran-amq-inject-bridge prefers queue over tmux when YARAN_PANE is set" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export YARAN_PANE="1"
  export AM_ME="bob"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "under-yaran" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # tmux must NOT have been called — YARAN_PANE wins over $TMUX.
  [ ! -s "$TMUX_LOG" ]
  # Body must be queued under bob/, not `.unrouted/`.
  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "under-yaran" "${files[0]}"
}

@test "yaran-amq-inject-bridge drops under-yaran wake when YARAN_PID is dead" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export YARAN_PANE="1"
  export YARAN_PID="999999999"
  export AM_ME="bob"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "stale-yaran" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  [ ! -s "$TMUX_LOG" ]
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null || false
}

# 13.7b — under yaran with AM_ROOT: bridge auto-drains and queues the
# actual message body instead of a "run amq drain" reminder.
@test "yaran-amq-inject-bridge auto-drains AMQ when under yaran with AM_ROOT" {
  install_fake_tmux
  install_fake_drain_amq
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  export YARAN_PANE="1"
  export AM_ME="bob"
  export AM_ROOT="$TEST_HOME/amq-root"
  AMQ_DRAIN_ARGV_LOG="$TEST_HOME/amq-drain.argv"
  export AMQ_DRAIN_ARGV_LOG

  run yaran-amq-inject-bridge "AMQ: message from alice - hello. Drain with: amq drain --include-body"
  [ "$status" -eq 0 ]
  [ ! -s "$TMUX_LOG" ]
  grep -Fxq -- "drain" "$AMQ_DRAIN_ARGV_LOG"
  grep -Fxq -- "--root" "$AMQ_DRAIN_ARGV_LOG"
  grep -Fxq -- "$AM_ROOT" "$AMQ_DRAIN_ARGV_LOG"
  grep -Fxq -- "--me" "$AMQ_DRAIN_ARGV_LOG"
  grep -Fxq -- "bob" "$AMQ_DRAIN_ARGV_LOG"
  grep -Fxq -- "--json" "$AMQ_DRAIN_ARGV_LOG"

  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  grep -Fq -- "[AMQ] Drained messages (JSON):" "${files[0]}"
  grep -Fq -- "drained-body" "${files[0]}"
  grep -Fq -- "Act on these AMQ messages now" "${files[0]}"
  # Unsafe Ctrl+C from the peer body was stripped before queueing.
  ! LC_ALL=C grep -q $'\003' "${files[0]}" || false
}

@test "yaran-amq-inject-bridge ignores an empty JSON drain with advisory stderr" {
  install_fake_drain_amq
  export YARAN_PANE="1"
  export AM_ME="bob"
  export AM_ROOT="$TEST_HOME/amq-root"
  export AMQ_DRAIN_ARGV_LOG="$TEST_HOME/amq-drain.argv"
  export AMQ_DRAIN_EMPTY=1

  run yaran-amq-inject-bridge "AMQ wake notification"

  [ "$status" -eq 0 ]
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null || false
}

@test "yaran-amq-inject-bridge drains downtime mail after the exact managed wake is ready" {
  install_fake_drain_amq
  export YARAN_PANE="1"
  export YARAN_PID="$$"
  export YARAN_AMQ_STARTUP_OWNER_PID="$$"
  export AM_ME="bob"
  export AM_ROOT="$TEST_HOME/amq-root"
  export AMQ_DRAIN_ARGV_LOG="$TEST_HOME/amq-drain.argv"
  mkdir -p "$AM_ROOT/agents/bob"
  printf '{"owner":{"pid":%s},"generation":"ready-generation"}\n' "$$" \
    >"$AM_ROOT/agents/bob/.wake.lock"
  printf '{"generation":"ready-generation"}\n' \
    >"$AM_ROOT/agents/bob/.wake.prepared"

  run yaran-amq-inject-bridge

  [ "$status" -eq 0 ]
  grep -Fxq -- "drain" "$AMQ_DRAIN_ARGV_LOG"
  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  grep -Fq -- "drained-body" "${files[0]}"
}

@test "strict signed delivery bypasses AMQ drain and queues the exact body" {
  install_fake_drain_amq
  export YARAN_PANE="1"
  export YARAN_AMQ_VERIFY="1"
  export AM_ME="bob"
  export AM_ROOT="$TEST_HOME/amq-root"
  AMQ_DRAIN_ARGV_LOG="$TEST_HOME/amq-drain.argv"
  export AMQ_DRAIN_ARGV_LOG
  local body msg files expected
  body=$'signed\tbody\nsecond line'
  msg=$(amq-send-signed --me alice --to bob --body "$body" --print-only)

  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  [ ! -s "$AMQ_DRAIN_ARGV_LOG" ]
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  expected="$BATS_TEST_TMPDIR/strict.expected"
  printf '%s' "$body" >"$expected"
  cmp "$expected" "${files[0]}"
}

# 13.7c — under yaran: raw Ctrl+C interrupt transport signals are a no-op.
@test "yaran-amq-inject-bridge drops raw ctrl-c interrupt payload under yaran" {
  export YARAN_PANE="1"
  export AM_ME="bob"
  run yaran-amq-inject-bridge $'\003'
  [ "$status" -eq 0 ]
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/*/*.msg" >/dev/null || false
}

# 13.8 — receiver path: AM_ME determines the subdirectory.
@test "yaran-amq-inject-bridge keys queue files on sanitised AM_ME" {
  unset TMUX
  export AM_ME="bob"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "addressed" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/bob/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  grep -Fxq -- "addressed" "${files[0]}"
}

# 13.9 — receiver sanitisation: uppercase + bad chars normalised to
# the same lowercase regex the wrappers apply, blocking path traversal.
@test "yaran-amq-inject-bridge sanitises AM_ME (uppercase, slashes, dots)" {
  unset TMUX
  # Mirrors the wrapper sanitisation: tr '[:upper:]' '[:lower:]' then
  # sed 's|[^a-z0-9_-]|-|g; s|^-\+||; s|-\+$||'. So "Feature/Login.v2"
  # becomes "feature-login-v2".
  export AM_ME="Feature/Login.v2"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "sanitised" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  local files
  collect_queue_files "$HOME/.local/share/dux-amq/inject-queue/feature-login-v2/*.msg"
  files=("${QUEUE_FILES[@]}")
  [ "${#files[@]}" -eq 1 ]
  # The unsanitised value MUST NOT exist as a directory — defence
  # against `..` or absolute paths sneaking in through AM_ME.
  [ ! -d "$HOME/.local/share/dux-amq/inject-queue/Feature/Login.v2" ]
}

# 13.10 — receiver sanitisation cannot escape the queue root.
@test "yaran-amq-inject-bridge rejects path-traversal AM_ME" {
  unset TMUX
  # `..` would map to `--` after sed, which then has leading dashes
  # stripped to empty. Empty receivers fall back to `.unrouted/`.
  export AM_ME="../../etc"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body "traversal" --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # The body landed somewhere INSIDE inject-queue/, not above it.
  ! compgen -G "$HOME/.local/share/dux-amq/inject-queue/../*.msg" >/dev/null || false
  # And not in any directory derived from the literal `../../etc`.
  [ ! -e "$HOME/.local/share/etc" ]
}

# 13.11 — skip mode (default): plain unsigned bodies are delivered as-is.
@test "yaran-amq-inject-bridge (skip mode) delivers unsigned body via tmux" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  # No YARAN_AMQ_VERIFY → default skip mode.
  run yaran-amq-inject-bridge "hello-from-legacy-amq-send"
  [ "$status" -eq 0 ]
  grep -Fxq -- "send-keys" "$TMUX_LOG"
  grep -Fxq -- "hello-from-legacy-amq-send" "$TMUX_LOG"
  grep -Fxq -- "Enter" "$TMUX_LOG"
}

# 13.12 — skip mode: YARAN2 envelope is unwrapped without MAC check.
@test "yaran-amq-inject-bridge (skip mode) unwraps YARAN2 envelope without verifying MAC" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  local msg bad
  msg=$(amq-send-signed --me alice --to bob --body "unwrap-me" --print-only)
  # Mangle the MAC so amq-receive-verify (in strict mode) WOULD reject
  # it. Skip mode must still deliver the inner body.
  # Tamper with the MAC in field 7. Skip mode must still decode field 6.
  IFS=$'\t' read -ra fields <<<"$msg"
  fields[6]="${fields[6]}TAMPERED"
  bad=$(printf '%s\t%s\t%s\t%s\t%s\t%s\t%s' "${fields[0]}" "${fields[1]}" "${fields[2]}" "${fields[3]}" "${fields[4]}" "${fields[5]}" "${fields[6]}")
  run yaran-amq-inject-bridge "$bad"
  [ "$status" -eq 0 ]
  grep -Fxq -- "send-keys" "$TMUX_LOG"
  grep -Fxq -- "unwrap-me" "$TMUX_LOG"
}

# 13.13 — skip mode: malformed YARAN2 (too few fields) falls back to raw.
@test "yaran-amq-inject-bridge (skip mode) treats malformed YARAN2 as raw body" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  # `YARAN2\t<sender>` only — five missing fields. Bridge should treat
  # the whole thing as a raw body rather than panic or drop.
  local broken=$'YARAN2\talice'
  run yaran-amq-inject-bridge "$broken"
  [ "$status" -eq 0 ]
  grep -Fxq -- "send-keys" "$TMUX_LOG"
  # Whole envelope visible in the log (skip mode delivered raw).
  grep -Fq -- "YARAN2" "$TMUX_LOG"
  grep -Fq -- "alice" "$TMUX_LOG"
}

# 13.14 — skip mode: empty argv is still a no-op (matches strict mode).
@test "yaran-amq-inject-bridge (skip mode) handles empty argv silently" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  run yaran-amq-inject-bridge ""
  [ "$status" -eq 0 ]
  [ ! -s "$TMUX_LOG" ]
}

# 13.15 — skip mode: body containing internal TABs survives YARAN2 decode.
@test "yaran-amq-inject-bridge (skip mode) preserves internal TABs in YARAN2 body" {
  install_fake_tmux
  export TMUX="/tmp/fake-tmux-socket,1234,0"
  local msg
  msg=$(amq-send-signed --me alice --to bob --body $'line1\tline2' --print-only)
  run yaran-amq-inject-bridge "$msg"
  [ "$status" -eq 0 ]
  # Both halves of the body should land in tmux send-keys.
  grep -Fq -- "line1" "$TMUX_LOG"
  grep -Fq -- "line2" "$TMUX_LOG"
}
