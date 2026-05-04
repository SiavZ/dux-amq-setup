#!/usr/bin/env bash
# shellcheck shell=bash
#
# path-encode.sh — replicate Claude Code's on-disk encoding of an absolute
# path into the directory name it uses under `~/.claude/projects/`.
#
# Verification (audit01 phase 04, run on this VM 2026-05-03 against
# claude 2.1.111):
#
#   /tmp/test-encoder/foo_bar.baz   →  -tmp-test-encoder-foo-bar-baz
#   /tmp/enc-test2/has space        →  -tmp-enc-test2-has-space
#   /tmp/enc-test2/Æneid            →  -tmp-enc-test2--neid     (Æ → 1 '-')
#   /tmp/enc-test2/a--b             →  -tmp-enc-test2-a--b      (runs preserved)
#   /tmp/enc-test2/a.b.c            →  -tmp-enc-test2-a-b-c
#   /tmp/enc3/a~b                   →  -tmp-enc3-a-b
#   /tmp/enc3/foo+bar               →  -tmp-enc3-foo-bar
#   /tmp/enc3/a@b                   →  -tmp-enc3-a-b
#   /tmp/enc3/123_abc-XYZ           →  -tmp-enc3-123-abc-XYZ    (case preserved)
#   /tmp/enc4/a__b                  →  -tmp-enc4-a--b
#   /tmp/enc4/a..b                  →  -tmp-enc4-a--b
#   /tmp/encu/日本                   →  -tmp-encu---             (each codepoint → 1 '-')
#
# Rule: every Unicode CODEPOINT that is not in [A-Za-z0-9-] is replaced
# with a single '-' (one-for-one, runs preserved, case preserved).
# The previous in-line encoder `s|/|-|g; s|_|-|g` silently disagreed
# with Claude Code on dots, spaces, unicode, and any other special
# chars — leading to seeding into the wrong project dir or a phantom
# dir Claude never reads.
#
# Switch to a public CLI surface (e.g. `claude config sessions-dir`)
# if Anthropic ships one.

# path_encode <absolute_path> → prints encoded form on stdout (no trailing newline).
path_encode() {
  local p="$1"
  # Prefer python3 (codepoint-correct for non-ASCII paths). Fall back to
  # `tr -c 'A-Za-z0-9-' '-'` which is byte-correct for ASCII paths but
  # over-replaces multi-byte UTF-8. ASCII is the common case.
  if command -v python3 >/dev/null 2>&1; then
    python3 -c '
import sys
s = sys.argv[1]
sys.stdout.write("".join(c if (c.isascii() and (c.isalnum() or c == "-")) else "-" for c in s))
' "$p"
  else
    LC_ALL=C printf '%s' "$p" | LC_ALL=C tr -c 'A-Za-z0-9-' '-'
  fi
}
