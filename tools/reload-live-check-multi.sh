#!/bin/sh
# Live end-to-end check of `reload-binary` with SEVERAL agents and the two
# failure paths, against a REAL dux binary (companion to reload-live-check.sh,
# which covers one agent in depth). Scratch DUX_HOME and scratch AMQ root, so
# nothing touches the user's real state. It checks:
#
#   - three printing agents (alpha, beta, gamma) are all adopted, same pids,
#     no duplicates, and EACH shows its own pre-reload transcript after it
#   - the reload reopens on the agent that was selected (gamma)
#   - refusal (binary not newer): no exec, agents untouched, output keeps
#     flowing
#   - failed exec (new binary not executable): reported, same dux, same
#     agents, output resumes, no handoff or sidecar files left behind
#
# Usage: tools/reload-live-check-multi.sh [path/to/dux]   (default: target/release/dux)
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

# THREE standalone agents in the same folder, each leaves its own marker.
REL=${WORK#"$HOME"/}
make_agent() {
  name=$1; before=$2
  keys s
  keys g
  mkdir -p "$WORK/proj/$name"; keys -l "$REL/proj/$name"
  keys Enter
  wait_for "the name prompt ($name)" 10 "tmux capture-pane -t $SESSION -p | grep -q 'Name standalone agent'"
  keys -l "$name"
  keys Enter
  wait_for "agent $name" 15 "[ \$(ps -Ao ppid,command | awk '\$1==$DUX_PID && \$2==\"sh\"' | wc -l) -gt $before ]"
  keys Enter; keys -l "marker-$name"
  keys Enter
  wait_for "marker-$name echoed" 15 "tmux capture-pane -t $SESSION -p | grep -q 'got:marker-$name'"
  keys 'C-]'; sleep 1; keys Escape; sleep 0.5
  # Focus stays on the agent pane after creation; go back to the agent list.
  keys BTab; sleep 0.5
}
make_agent alpha 0
make_agent beta 1
make_agent gamma 2
PIDS_BEFORE=$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && $3=="sh" {print $1}' | sort | tr '\n' ' ')
echo "before: dux=$DUX_PID agents=$PIDS_BEFORE"
sleep 2
# REFUSAL 1: no newer binary on disk. Nothing may change.
keys C-p
keys -l "reload-binary"
keys Enter
wait_for "the refusal message" 15 "tmux capture-pane -t $SESSION -p | grep -q 'Already running the newest dux'"
pass "refused: no newer build"
ps -p $DUX_PID -o command= | grep -q -- --reload-handoff && fail "exec happened on refusal" || pass "no exec on refusal"
[ "$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && $3=="sh" {print $1}' | sort | tr '\n' ' ')" = "$PIDS_BEFORE" ] && pass "agents untouched by refusal" || fail "agents changed on refusal"
screen | grep -q 'got:marker-gamma' && pass "screen still live after refusal" || fail "screen lost after refusal"
# Agents must keep updating after a refusal (readers resumed / never stopped).
T1=$(last_tick); sleep 3; T2=$(last_tick)
[ -n "$T1" ] && [ "$T2" -gt "$T1" ] && pass "agent output keeps flowing after refusal ($T1 -> $T2)" || fail "output stalled after refusal ($T1 -> $T2)"
# FAILURE 2: the exec itself fails (new binary on disk is not executable).
# Everything is prepared (readers stopped, sidecars written) and then must be
# undone: same process, same agents, output flowing again, no files left.
cp "$WORK/dux" "$WORK/dux.good"
# A truncated Mach-O header: the kernel rejects it (ENOEXEC) and, unlike a
# text file, it is not handed to /bin/sh, so exec really returns an error.
# Not executable: execve fails with EACCES before the point of no return,
# which is the failure dux can actually recover from.
cp "$WORK/dux" "$WORK/dux.bad"; chmod -x "$WORK/dux.bad"
mv "$WORK/dux.bad" "$WORK/dux"
sleep 1
keys C-p
keys -l "reload-binary"
keys Enter
wait_for "the failed-exec message" 15 "tmux capture-pane -t $SESSION -p | grep -q 'Reload failed'"
pass "failed exec reported"
[ "$(pgrep -f "^$WORK/dux" | head -1)" = "$DUX_PID" ] && pass "same dux after failed exec" || fail "dux pid changed after failed exec"
[ "$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && $3=="sh" {print $1}' | sort | tr '\n' ' ')" = "$PIDS_BEFORE" ] && pass "agents untouched by failed exec" || fail "agents changed on failed exec"
T1=$(last_tick); sleep 3; T2=$(last_tick)
[ -n "$T1" ] && [ "$T2" -gt "$T1" ] && pass "output resumes after failed exec ($T1 -> $T2)" || fail "output stalled after failed exec ($T1 -> $T2)"
ls "$WORK/home"/reload-handoff-* "$WORK/home"/reload-repaint-* >/dev/null 2>&1 && fail "files left after failed exec" || pass "no handoff/sidecar left after failed exec"
mv "$WORK/dux.good" "$WORK/dux"; chmod +x "$WORK/dux"
touch "$WORK/dux"
sleep 1
keys C-p
keys -l "reload-binary"
keys Enter
wait_for "the reload to exec" 20 "ps -p $DUX_PID -o command= | grep -q -- --reload-handoff"
wait_for "the reloaded TUI" 20 "grep -q 'reload: adopted' '$WORK/home/dux.log'"
grep -q "reload: adopted 3 of 3" "$WORK/home/dux.log" && pass "adopted 3 of 3" || fail "$(grep 'reload: adopted' "$WORK/home/dux.log")"
sleep 2
PIDS_AFTER=$(ps -Ao pid,ppid,command | awk -v p="$DUX_PID" '$2==p && $3=="sh" {print $1}' | sort | tr '\n' ' ')
[ "$PIDS_BEFORE" = "$PIDS_AFTER" ] && pass "same 3 agent pids, no duplicates ($PIDS_AFTER)" || fail "agents changed: $PIDS_BEFORE -> $PIDS_AFTER"
# The selected agent (gamma, the last one used) must be on screen with its transcript.
screen | grep -q 'got:marker-gamma' && pass "selected agent gamma shows its pre-reload transcript" || fail "gamma transcript missing"
# Every other agent must have its own transcript too: walk to each and look.
for name in alpha beta; do
  found=0
  for step in 1 2 3 4 5 6; do
    keys k
    sleep 0.5
    if screen | grep -q "got:marker-$name"; then found=1; break; fi
  done
  [ $found = 1 ] && pass "agent $name shows its pre-reload transcript" || fail "agent $name transcript missing"
done
ls "$WORK/home"/reload-repaint-* >/dev/null 2>&1 && fail "repaint sidecars left behind" || pass "all repaint sidecars removed"
echo "PASS"
