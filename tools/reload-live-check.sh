#!/bin/sh
# Live end-to-end check of `reload-binary` against a REAL dux binary.
#
# The cargo e2e test (crates/dux-core/tests/reload_handoff_e2e.rs) proves the
# handoff mechanism across a real exec, but it drives a stand-in program, not
# dux. This drives the actual TUI in tmux: start dux with a scratch DUX_HOME
# (and a scratch AMQ root, so nothing registers in the user's real
# ~/.local/state/amq), create an agent whose provider both prints a tick every
# second AND echoes input, make the binary newer, run `reload-binary` from the
# palette, and check what the reload promises:
#
#   - the dux pid is unchanged (exec keeps the process)
#   - the agent pid is unchanged and is still a child of dux
#   - no second agent process was spawned beside it
#   - the reload happens WHILE the agent is printing (the tick loop never
#     stops; an older dux would refuse this as "agent is mid-turn")
#   - the agent's output from before the reload is still on screen after it
#     (the terminal, scrollback included, crossed the exec)
#   - ticks printed before the reload are still visible and newer ticks keep
#     arriving (nothing was lost and the stream continued)
#   - the agent still answers typed input through the adopted pty
#   - the session row is persisted as active
#   - the handoff file and its repaint sidecars are gone
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
# The launched dux must not register inboxes (or anything else) in the user's
# real AMQ root: both env names are pointed at a scratch directory that dies
# with $WORK. Passed again explicitly inside tmux, because the tmux server
# inherits its own environment, not this script's.
export AMQ_GLOBAL_ROOT="$WORK/amq" AM_ROOT="$WORK/amq"
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
last_tick() { screen | grep -o 'tick-[0-9]*' | sed 's/tick-//' | sort -n | tail -1; }
keys() { tmux send-keys -t "$SESSION" "$@"; sleep 0.3; }

mkdir -p "$WORK/home" "$WORK/proj" "$WORK/amq"
cp "$DUX_SRC" "$WORK/dux"
git -C "$WORK/proj" init -q
git -C "$WORK/proj" -c user.email=x@x -c user.name=x commit -q --allow-empty -m init

DUX_HOME="$WORK/home" "$WORK/dux" config regenerate --yes >/dev/null
CFG="$WORK/home/config.toml"
# A provider that is BOTH always printing (the reload must happen mid-stream)
# and answering (typed input must survive the adopted pty). A non-empty
# `resume_args` makes it auto-reopen ELIGIBLE, so a boot restore that ignored
# the adopted agent would spawn a second one; `sh -c` takes the extra word as
# `$0`, so the resumed command is the same program.
sed -i.bak \
  -e 's/^provider = "claude"$/provider = "cat"/' \
  -e 's/^auto_reopen_agents = false$/auto_reopen_agents = true/' \
  -e 's/^disable_automated_welcome_screen = false$/disable_automated_welcome_screen = true/' \
  -e 's/^disable_automated_whats_new_screen = false$/disable_automated_whats_new_screen = true/' \
  "$CFG"
cat >>"$CFG" <<'EOF'

[providers.cat]
command = "sh"
args = ["-c", "while true; do echo \"tick-$(date +%s)\"; sleep 1; done & while IFS= read -r line; do echo \"got:$line\"; done"]
resume_args = ["resumed"]
resume_wait_timeout_ms = 0
EOF

tmux new-session -d -s "$SESSION" -x 200 -y 50 -c "$WORK/proj" \
  "env DUX_HOME='$WORK/home' AMQ_GLOBAL_ROOT='$WORK/amq' AM_ROOT='$WORK/amq' '$WORK/dux'; sleep 600"
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
wait_for "the agent" 15 "ps -Ao ppid,command | awk '\$1==$DUX_PID && \$2==\"sh\"' | grep -q 'while true'"
AGENT_PID=$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && $3=="sh" {print $1}')

# While the pane is still focused and the agent is printing: leave a marker in
# its transcript, and remember the newest tick already on screen.
keys -l "marker-before-reload"
keys Enter
wait_for "the marker to be echoed" 15 "tmux capture-pane -t $SESSION -p | grep -q 'got:marker-before-reload'"
wait_for "a first tick" 15 "tmux capture-pane -t $SESSION -p | grep -q 'tick-'"
BEFORE_TICK=$(last_tick)
[ -n "$BEFORE_TICK" ] || fail "no tick on screen before the reload"
# Release keyboard focus from the agent so the palette key reaches dux.
keys 'C-]'
sleep 2 # let a few more ticks print, so the reload happens mid-stream
echo "before: dux=$DUX_PID agent=$AGENT_PID tick=$BEFORE_TICK"

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
COUNT=$(ps -Ao ppid,command | awk -v p="$DUX_PID" '$1==p && $2=="sh"' | wc -l | tr -d ' ')
[ "$COUNT" = 1 ] && pass "exactly one agent process" || fail "$COUNT agent processes (duplicate relaunch?)"
grep -q "reload: adopted 1 of 1" "$WORK/home/dux.log" && pass "adopted 1 of 1" || fail "adoption count wrong"
STATUS=$(sqlite3 "$WORK/home/sessions.sqlite3" "select status from agent_sessions")
[ "$STATUS" = active ] && pass "session persisted as active" || fail "session status is $STATUS"
ls "$WORK/home"/reload-handoff-* >/dev/null 2>&1 && fail "handoff file left behind" || pass "handoff file removed"
ls "$WORK/home"/reload-repaint-* >/dev/null 2>&1 && fail "repaint sidecar left behind" || pass "repaint sidecar removed"

# The transcript must have crossed the exec: the marker typed before the reload
# and the ticks that predate it are still on the adopted terminal.
screen | grep -q 'got:marker-before-reload' \
  && pass "pre-reload transcript still visible" \
  || fail "pre-reload transcript vanished (terminal state not carried)"
screen | grep -q "tick-$BEFORE_TICK" \
  && pass "tick from before the reload still visible" \
  || fail "tick-$BEFORE_TICK lost"

# And the stream must have continued: a tick NEWER than the last pre-reload one.
wait_for "a tick newer than the reload" 30 \
  "[ \"\$(tmux capture-pane -t '$SESSION' -p | grep -o 'tick-[0-9]*' | sed 's/tick-//' | sort -n | tail -1)\" -gt $BEFORE_TICK ]"
pass "agent kept printing through the reload"

keys Enter
keys -l "ping-after-reload"
keys Enter
wait_for "the agent to echo" 10 "tmux capture-pane -t $SESSION -p | grep -q 'got:ping-after-reload'"
pass "agent answers through the adopted pty"
echo "PASS"
