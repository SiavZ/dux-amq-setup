#!/bin/sh
# A fake agent provider with deterministic screenshot fixtures. Normal preview
# use defaults to a live stream so working and idle transitions remain visible.
fixture="${DUX_FAKE_FIXTURE:-live}"

case "$fixture" in
  steady)
    printf '%s\n' \
      'Review complete. The retry path is covered.' \
      'Files checked: src/client.rs, src/retry.rs, tests/retry.rs' \
      'Waiting for the next instruction.'
    while :; do sleep 60; done
    ;;
  working)
    printf '%s\n' \
      'Implementing bounded retries for failed API requests.' \
      '✓ Read the existing request flow' \
      '✓ Added the retry policy' \
      '→ Running focused tests'
    i=1
    while :; do
      printf '  test batch %02d passed\n' "$i"
      i=$((i + 1))
      sleep 1
    done
    ;;
  attention)
    printf '%s\n' 'Implementation is ready for review.'
    printf '\033]9;Review requested for the retry policy\007'
    while :; do sleep 60; done
    ;;
  attention-delayed)
    # The same bell, rung late, for the terminal UI. There the selected agent's
    # pane is always on screen and looking at a pane is what clears the flag, so
    # a bell rung at spawn is cleared by the very act of creating the agent. A
    # browser scene has no such problem (it relights an agent while the page is
    # on the home screen), which is why only this one waits. Long enough for a
    # journey to create the agent and move the selection off it.
    printf '%s\n' 'Implementation is ready for review.'
    sleep 20
    printf '\033]9;Review requested for the retry policy\007'
    while :; do sleep 60; done
    ;;
  quit-on-command)
    # A clean exit on demand, for measuring what dux does when a provider ends
    # the way a user quitting one ends it: status 0, after real typed input.
    echo 'fake-agent: type "quit" and press Enter to end this session cleanly.'
    while IFS= read -r line; do
      case "$line" in
        quit*) exit 0 ;;
      esac
      printf 'fake-agent: you typed %s\n' "$line"
    done
    exit 0
    ;;
  failure)
    printf '%s\n' 'Error: the fixture dependency could not be resolved.' >&2
    exit 2
    ;;
  burst)
    # Measurement fixture: print a known number of lines of a known width as
    # fast as the PTY will take them, then idle. The live fixture streams at
    # under two lines a second, which is fine for looking at a working pane and
    # useless for filling a 10,000-line scrollback. Not a screenshot scene: it
    # exists so a memory measurement can put a terminal into a stated state.
    #   DUX_FAKE_BURST_LINES  how many lines to print (default 10000)
    #   DUX_FAKE_BURST_COLS   printed width of each line (default 80)
    #   DUX_FAKE_BURST_DELAY  seconds to idle first, so a measurement can put
    #                         the terminal at a chosen width before anything is
    #                         printed into it (default 0)
    lines="${DUX_FAKE_BURST_LINES:-10000}"
    cols="${DUX_FAKE_BURST_COLS:-80}"
    sleep "${DUX_FAKE_BURST_DELAY:-0}"
    awk -v n="$lines" -v w="$cols" 'BEGIN {
      filler = "abcdefghijklmnopqrstuvwxyz0123456789"
      while (length(filler) < w) filler = filler filler
      for (i = 0; i < n; i++) {
        pre = sprintf("burst %08d ", i)
        pad = w - length(pre)
        if (pad < 1) pad = 1
        printf "%s%s\n", pre, substr(filler, 1, pad)
      }
    }'
    echo "fake-agent: burst of $lines lines at $cols columns complete."
    while :; do sleep 60; done
    ;;
  live)
    i=0
    echo "fake-agent: streaming output so this session reads as Working."
    echo "fake-agent: close this tab or interrupt to return the agent to Idle."
    while true; do
      printf 'fake-agent working... line %d @ %s\n' "$i" "$(date +%H:%M:%S)"
      i=$((i + 1))
      sleep 0.6
    done
    ;;
  *)
    echo "fake-agent: unknown fixture: $fixture" >&2
    exit 64
    ;;
esac
