#!/bin/sh
# Live end-to-end check of `reload-binary` against a REAL dux binary.
#
# The cargo e2e test (crates/dux-core/tests/reload_handoff_e2e.rs) proves the
# handoff mechanism across a real exec, but it drives a stand-in program, not
# dux. This drives the actual TUI in tmux: start dux with a scratch DUX_HOME,
# create an agent whose provider is a plain `cat`, make the binary newer, run
# `reload-binary` from the palette, and check what the reload promises:
#
#   - the dux pid is unchanged (exec keeps the process)
#   - the agent pid is unchanged and is still a child of dux
#   - no second agent process was spawned beside it
#   - the agent still answers through the adopted pty
#   - the session row is persisted as active
#   - the handoff file is gone
#
# Usage: tools/reload-live-check.sh [path/to/dux]   (default: target/release/dux)
# Needs: tmux, sqlite3, git. Exits non-zero on the first failed check.
#
# Every wait polls for the state it needs rather than sleeping a fixed time, so
# a slow machine takes longer instead of failing.

set -eu

DUX_SRC=${1:-target/release/dux}
[ -x "$DUX_SRC" ] || { echo "no dux binary at $DUX_SRC (run cargo build --release)"; exit 2; }
for tool in tmux sqlite3 git; do
  command -v "$tool" >/dev/null || { echo "missing $tool"; exit 2; }
done

# Under $HOME on purpose: dux's folder prompt starts at the home directory.
WORK=$(mktemp -d "$HOME/.dux-reload-check.XXXXXX")
SESSION="duxreload-$$"
DUX_PID=""
cleanup() {
  [ -n "$DUX_PID" ] && kill -9 "$DUX_PID" 2>/dev/null || true
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

fail() { echo "FAIL: $*"; tmux capture-pane -t "$SESSION" -p | tail -5; exit 1; }
pass() { echo "ok:   $*"; }

# wait_for <description> <seconds> <shell condition>
wait_for() {
  what=$1; secs=$2; shift 2
  i=0
  while ! sh -c "$*" >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -gt $((secs * 5)) ] && fail "timed out waiting for $what"
    sleep 0.2
  done
}
screen() { tmux capture-pane -t "$SESSION" -p; }
keys() { tmux send-keys -t "$SESSION" "$@"; sleep 0.3; }

mkdir -p "$WORK/home" "$WORK/proj"
cp "$DUX_SRC" "$WORK/dux"
git -C "$WORK/proj" init -q
git -C "$WORK/proj" -c user.email=x@x -c user.name=x commit -q --allow-empty -m init

DUX_HOME="$WORK/home" "$WORK/dux" config regenerate --yes >/dev/null
CFG="$WORK/home/config.toml"
# An idle provider: a mid-turn agent (one that keeps printing) is refused by
# design. A non-empty `resume_args` makes it auto-reopen ELIGIBLE, so a boot
# restore that ignored the adopted agent would spawn a second one; `sh -c`
# takes the extra word as `$0`, so the resumed command is the same `cat`.
sed -i.bak \
  -e 's/^provider = "claude"$/provider = "cat"/' \
  -e 's/^auto_reopen_agents = false$/auto_reopen_agents = true/' \
  -e 's/^disable_automated_welcome_screen = false$/disable_automated_welcome_screen = true/' \
  -e 's/^disable_automated_whats_new_screen = false$/disable_automated_whats_new_screen = true/' \
  "$CFG"
printf '\n[providers.cat]\ncommand = "sh"\nargs = ["-c", "exec cat"]\nresume_args = ["resumed"]\nresume_wait_timeout_ms = 0\n' >>"$CFG"

tmux new-session -d -s "$SESSION" -x 200 -y 50 -c "$WORK/proj" \
  "env DUX_HOME='$WORK/home' '$WORK/dux'; sleep 600"
wait_for "dux to start" 20 "pgrep -f '^$WORK/dux\$'"
DUX_PID=$(pgrep -f "^$WORK/dux\$")
wait_for "the TUI" 20 "tmux capture-pane -t $SESSION -p | grep -q 'Agents (0)'"

# Standalone agent in the scratch project: s, go-to, path relative to $HOME.
keys s
keys g
REL=${WORK#"$HOME"/}
keys -l "$REL/proj"
keys Enter
wait_for "the name prompt" 10 "tmux capture-pane -t $SESSION -p | grep -q 'Name standalone agent'"
keys -l "probe"
keys Enter
wait_for "the agent" 15 "ps -Ao ppid,command | awk '\$1==$DUX_PID && \$2==\"cat\"' | grep -q cat"
AGENT_PID=$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && $3=="cat" {print $1}')
# Release keyboard focus from the agent so the palette key reaches dux.
keys 'C-]'
echo "before: dux=$DUX_PID agent=$AGENT_PID"

touch "$WORK/dux"
sleep 1
keys C-p
keys -l "reload-binary"
keys Enter
wait_for "the reload to exec" 20 "ps -p $DUX_PID -o command= | grep -q -- --reload-handoff"
wait_for "the reloaded TUI" 20 "grep -q 'reload: adopted' '$WORK/home/dux.log'"

NEW_PID=$(pgrep -f "^$WORK/dux --reload-handoff" || true)
[ "$NEW_PID" = "$DUX_PID" ] && pass "dux pid unchanged ($DUX_PID)" || fail "dux pid changed: $DUX_PID -> $NEW_PID"
ps -p "$AGENT_PID" >/dev/null && pass "agent $AGENT_PID still running" || fail "agent $AGENT_PID died"
PARENT=$(ps -p "$AGENT_PID" -o ppid= | tr -d ' ')
[ "$PARENT" = "$DUX_PID" ] && pass "agent is still dux's child" || fail "agent parent is $PARENT"
sleep 2 # give a (buggy) auto-reopen time to show up
COUNT=$(ps -Ao ppid,command | awk -v p="$DUX_PID" '$1==p && $2=="cat"' | wc -l | tr -d ' ')
[ "$COUNT" = 1 ] && pass "exactly one agent process" || fail "$COUNT agent processes (duplicate relaunch?)"
grep -q "reload: adopted 1 of 1" "$WORK/home/dux.log" && pass "adopted 1 of 1" || fail "adoption count wrong"
STATUS=$(sqlite3 "$WORK/home/sessions.sqlite3" "select status from agent_sessions")
[ "$STATUS" = active ] && pass "session persisted as active" || fail "session status is $STATUS"
ls "$WORK/home"/reload-handoff-* >/dev/null 2>&1 && fail "handoff file left behind" || pass "handoff file removed"

keys Enter
keys -l "ping-after-reload"
keys Enter
wait_for "the agent to echo" 10 "tmux capture-pane -t $SESSION -p | grep -c ping-after-reload | grep -q '^[2-9]'"
pass "agent answers through the adopted pty"
echo "PASS"
