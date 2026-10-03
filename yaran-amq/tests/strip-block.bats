#!/usr/bin/env bats
#
# Audit02 P0-G regression: the legacy CLAUDE.md branch of `strip_block`
# used to set s=1 on the `## Multi-agent environment (AMQ + yaran)`
# heading and never reset it — anything appended after the AMQ stanza
# (a user's own `## Notes` etc.) was deleted to EOF.
#
# This test exercises `strip_block` directly as a unit. We extract its
# definition out of install.sh (rather than sourcing the whole file,
# which would trip the `[[ -d /data ]]` preflight and start downloading
# things) and feed it fixtures.

load 'lib/setup'

setup() {
  setup_isolated_home
  REPO_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  # Extract the helper functions (`say`, `warn`, `ok`, `strip_block`)
  # from install.sh — every line up to (but not including) the
  # preflight section header.
  HELPERS="$TEST_HOME/strip_block_helpers.sh"
  # Strip `set -euo pipefail` (sourcing it would mutate the bats test
  # shell's options and cause unrelated assertions to short-circuit) and
  # cut everything from the preflight section onward.
  awk '
    /^# 1\. preflight/ { exit }
    /^set -euo pipefail$/ { next }
    { print }
  ' "$REPO_ROOT/yaran-amq/install.sh" > "$HELPERS"
  # shellcheck disable=SC1090
  source "$HELPERS"
}

teardown() {
  teardown_isolated_home
}

@test "P0-G: legacy md branch preserves user content after the AMQ heading" {
  local f="$TEST_HOME/CLAUDE.md"
  cat > "$f" <<'MD'
## Multi-agent environment (AMQ + dux)
old content from pre-phase-12 install
should be removed
## My personal notes
DO NOT DELETE
MD
  strip_block "$f" md
  grep -q "DO NOT DELETE" "$f"
  grep -q "## My personal notes" "$f"
  ! grep -q "old content from pre-phase-12 install" "$f" || false
}

@test "P0-G: versioned md markers are still stripped" {
  local f="$TEST_HOME/CLAUDE.md"
  cat > "$f" <<'MD'
# Pre-existing top heading

<!-- >>> yaran-amq v0.0.9 >>> -->
old version block
<!-- <<< yaran-amq v0.0.9 <<< -->

## User notes
keep me
MD
  strip_block "$f" md
  ! grep -q "old version block" "$f" || false
  grep -q "keep me" "$f"
}

@test "P0-G: explicit end-sentinel resets the legacy block" {
  local f="$TEST_HOME/CLAUDE.md"
  cat > "$f" <<'MD'
## Multi-agent environment (AMQ + dux)
legacy stanza
<!-- end dux-amq legacy -->

free text outside any heading
MD
  strip_block "$f" md
  ! grep -q "legacy stanza" "$f" || false
  grep -q "free text outside any heading" "$f"
}

@test "rebrand strips old and new shell blocks idempotently" {
  local f="$TEST_HOME/bashrc"
  cat >"$f" <<'SH'
user_before=yes
# >>> dux-amq v0.1.0 >>>
old_block=yes
# <<< dux-amq v0.1.0 <<<
# === dux + AMQ ===
legacy_block=yes
# === end dux + AMQ ===
# >>> yaran-amq v0.1.0 >>>
new_block=yes
# <<< yaran-amq v0.1.0 <<<
user_after=yes
SH
  strip_block "$f" sh
  cp "$f" "$f.once"
  strip_block "$f" sh
  cmp "$f" "$f.once"
  grep -q '^user_before=yes$' "$f"
  grep -q '^user_after=yes$' "$f"
  ! grep -q 'block=yes' "$f" || false
}

@test "rebrand strips historical md markers without deleting user content" {
  local f="$TEST_HOME/CLAUDE.md"
  cat >"$f" <<'MD'
# User title
<!-- >>> dux-amq v0.1.0 >>> -->
old block
<!-- <<< dux-amq v0.1.0 <<< -->
<!-- >>> yaran-amq v0.1.0 >>> -->
new block
<!-- <<< yaran-amq v0.1.0 <<< -->
## My notes
keep me
MD
  strip_block "$f" md
  cp "$f" "$f.once"
  strip_block "$f" md
  cmp "$f" "$f.once"
  ! grep -q 'block' "$f" || false
  grep -q 'keep me' "$f"
}
