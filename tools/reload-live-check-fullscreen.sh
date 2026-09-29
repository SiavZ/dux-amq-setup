#!/bin/sh
# Live reload check: a FULL-SCREEN (alternate screen) agent, like Claude Code or
# Codex, plus a companion terminal opened with `o`, against a REAL dux binary.
# Scratch DUX_HOME and scratch AMQ root, so nothing touches the user's state.
#
#   - both the agent and the terminal are adopted (2 of 2), same pids
#   - the terminal is still attached to its agent
#   - the agent's full-screen frame is drawn after the reload (not blank) and
#     it keeps redrawing
#   - the terminal keeps its transcript and still answers typed commands
#
# Usage: tools/reload-live-check-fullscreen.sh [path/to/dux]
set -eu
DUX_SRC=${1:-target/release/dux}
[ -x "$DUX_SRC" ] || { echo "no dux binary at $DUX_SRC"; exit 2; }
WORK=$(mktemp -d "$HOME/.dux-reload-check.XXXXXX")
SESSION="duxalt-$$"
DUX_PID=""
export AMQ_GLOBAL_ROOT="$WORK/amq" AM_ROOT="$WORK/amq"
cleanup() {
  [ -n "$DUX_PID" ] && kill -9 "$DUX_PID" 2>/dev/null || true
  tmux kill-session -t "$SESSION" 2>/dev/null || true
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM
fail() { echo "FAIL: $*"; tmux capture-pane -t "$SESSION" -p | grep -v '^│ *│ *│*$' | tail -12; exit 1; }
pass() { echo "ok:   $*"; }
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

mkdir -p "$WORK/home" "$WORK/proj" "$WORK/amq"
cp "$DUX_SRC" "$WORK/dux"
cat >"$WORK/alt-agent.sh" <<'AGENT'
#!/bin/sh
# Full-screen (alternate screen) stand-in for a TUI agent like Claude Code:
# enters the alt screen, draws a fixed frame once, then redraws a counter in
# place. Nothing scrolls, so after a reload the frame is only visible if the
# screen itself was carried over (the old scrollback-only view would be blank).
printf '\033[?1049h\033[2J\033[H'
printf 'ALT-FRAME-TOP drawn once\n'
printf 'ALT-FRAME-BODY line two\n'
i=0
while true; do
  i=$((i+1))
  printf '\033[5;1Hcounter-%s   ' "$i"
  sleep 1
done
AGENT
chmod +x "$WORK/alt-agent.sh"
git -C "$WORK/proj" init -q
git -C "$WORK/proj" -c user.email=x@x -c user.name=x commit -q --allow-empty -m init
DUX_HOME="$WORK/home" "$WORK/dux" config regenerate --yes >/dev/null
CFG="$WORK/home/config.toml"
sed -i.bak \
  -e 's/^provider = "claude"$/provider = "cat"/' \
  -e 's/^auto_reopen_agents = false$/auto_reopen_agents = true/' \
  -e 's/^disable_automated_welcome_screen = false$/disable_automated_welcome_screen = true/' \
  -e 's/^disable_automated_whats_new_screen = false$/disable_automated_whats_new_screen = true/' \
  "$CFG"
printf '\n[providers.cat]\ncommand = "%s"\nargs = []\nresume_args = ["resumed"]\nresume_wait_timeout_ms = 0\n' "$WORK/alt-agent.sh" >>"$CFG"

tmux new-session -d -s "$SESSION" -x 200 -y 50 -c "$WORK/proj" \
  "env DUX_HOME='$WORK/home' AMQ_GLOBAL_ROOT='$WORK/amq' AM_ROOT='$WORK/amq' '$WORK/dux'; sleep 600"
wait_for "dux to start" 20 "pgrep -f '^$WORK/dux\$'"
DUX_PID=$(pgrep -f "^$WORK/dux\$")
wait_for "the TUI" 20 "tmux capture-pane -t $SESSION -p | grep -q 'Agents (0)'"

REL=${WORK#"$HOME"/}
keys s
keys g
keys -l "$REL/proj"
keys Enter
wait_for "the name prompt" 10 "tmux capture-pane -t $SESSION -p | grep -q 'Name standalone agent'"
keys -l "fullscreen"
keys Enter
wait_for "the alt frame" 15 "tmux capture-pane -t $SESSION -p | grep -q 'ALT-FRAME-TOP'"
wait_for "the counter" 10 "tmux capture-pane -t $SESSION -p | grep -q 'counter-'"
keys 'C-]'; sleep 1; keys Escape; sleep 0.5; keys BTab; sleep 0.5
AGENT_PID=$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && /alt-agent/ {print $1}' | head -1)
[ -n "$AGENT_PID" ] || fail "could not find the agent pid"

# Put the cursor on the agent row (a group header sits above it).
screen | grep -q "fullscreen (cat)" || fail "agent row missing"
keys g; keys j; keys j
keys o
wait_for "a companion terminal" 15 "tmux capture-pane -t $SESSION -p | grep -q '1 terminal'"
sleep 2
keys Enter; sleep 0.5
keys -l "echo TERM-MARKER-before"
keys Enter
wait_for "the terminal marker" 10 "tmux capture-pane -t $SESSION -p | grep -q 'TERM-MARKER-before'"
# Match the shell itself: dux also forks short-lived helpers (`gh auth status`
# probes), and picking "any other child" sometimes caught one of those, which
# had exited by the post-reload check and read as a lost terminal.
TERM_SHELL=$(basename "${SHELL:-/bin/sh}")
SHELL_PID=$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" -v a="$AGENT_PID" -v s="$TERM_SHELL" \
  '$2==p && $1!=a && ($3 ~ ("(^|/|-)" s "$")) {print $1}' | head -1)
[ -n "$SHELL_PID" ] || fail "could not find the terminal shell pid"
echo "before: dux=$DUX_PID agent=$AGENT_PID shell=$SHELL_PID"
keys 'C-]'; sleep 1; keys C-g; sleep 1
screen | grep -q '╭ Terminal' && fail "terminal overlay did not minimize"

touch "$WORK/dux"; sleep 1
keys C-p
keys -l "reload-binary"
keys Enter
wait_for "the reload to exec" 20 "ps -p $DUX_PID -o command= | grep -q -- --reload-handoff"
wait_for "the reloaded TUI" 20 "grep -q 'reload: adopted' '$WORK/home/dux.log'"
grep -q "reload: adopted 2 of 2" "$WORK/home/dux.log" && pass "adopted agent + terminal (2 of 2)" || fail "$(grep 'reload: adopted' "$WORK/home/dux.log")"
ps -p "$AGENT_PID" >/dev/null && pass "alt-screen agent still running" || fail "agent died"
ps -p "$SHELL_PID" >/dev/null && pass "companion shell still running" || fail "companion shell gone"
sleep 1
screen | grep -q "1 terminal" && pass "terminal still attached to its agent" || fail "terminal no longer shown on the agent"
screen | grep -q 'ALT-FRAME-TOP' && pass "full-screen frame drawn after reload (not blank)" || fail "alt-screen frame missing after reload"
C1=$(screen | grep -o 'counter-[0-9]*' | head -1 | sed 's/counter-//'); sleep 3
C2=$(screen | grep -o 'counter-[0-9]*' | head -1 | sed 's/counter-//')
[ -n "$C1" ] && [ -n "$C2" ] && [ "$C2" -gt "$C1" ] && pass "alt-screen agent keeps redrawing ($C1 -> $C2)" || fail "counter not advancing ($C1 -> $C2)"
keys t
sleep 2
keys Enter; sleep 0.5
screen | grep -q 'TERM-MARKER-before' && pass "companion terminal transcript kept" || fail "terminal transcript lost"
keys -l "echo TERM-MARKER-after"
keys Enter
wait_for "the terminal to answer" 10 "tmux capture-pane -t $SESSION -p | grep -q 'TERM-MARKER-after'"
pass "companion terminal answers after reload"
echo "PASS"
