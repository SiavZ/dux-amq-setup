#!/usr/bin/env bats
#
# Phase 00 smoke test: verifies `lib/setup.bash` sources cleanly and
# its helpers behave as documented. Phases 02+ will add their own
# .bats files alongside this one.

load 'lib/setup'

setup() {
  setup_isolated_home
}

teardown() {
  teardown_isolated_home
}

@test "setup_isolated_home creates a fresh \$HOME under /tmp" {
  [ -n "$TEST_HOME" ]
  [ -d "$TEST_HOME" ]
  [ "$HOME" = "$TEST_HOME" ]
  case "$TEST_HOME" in
    /tmp/*|/var/folders/*|"$TMPDIR"*) ;;
    *) printf 'TEST_HOME=%s is not under a tmp prefix\n' "$TEST_HOME" >&2; return 1 ;;
  esac
}

@test "setup_isolated_home prepends tests/fakes to PATH" {
  case "$PATH" in
    "$BATS_TEST_DIRNAME/fakes":*) ;;
    *) printf 'PATH does not start with fakes/: %s\n' "$PATH" >&2; return 1 ;;
  esac
}

@test "teardown_isolated_home removes \$TEST_HOME and is idempotent" {
  local saved="$TEST_HOME"
  teardown_isolated_home
  [ ! -d "$saved" ]
  [ -z "${TEST_HOME:-}" ]
  # Second call must not error.
  teardown_isolated_home
}

# Audit01 P2-10 regression guard: bare `cd -` (return-to-OLDPWD) is unreliable
# under `bash --posix`, and `echo "$PWD"` mangles paths starting with `-` or
# containing `\`. Phase 01's mktemp+trap and Phase 04's printf rewrites removed
# the existing offenders. This test pins the absence so future PRs don't
# silently reintroduce either pattern.
@test "no bare 'cd -' in overlay shell (P2-10 regression guard)" {
  local overlay_root
  overlay_root="$(cd -- "$BATS_TEST_DIRNAME/.." && pwd)"
  # Match `cd -` followed by whitespace, end-of-line, or `>`/`|` redirection.
  # `cd --` is fine (option terminator); `cd -P` / `cd -L` are fine.
  if grep -REn 'cd -([[:space:]]|$|>|\|)' \
       "$overlay_root/install.sh" \
       "$overlay_root/wrappers" \
       "$overlay_root/lib" \
       "$overlay_root/scripts" \
       "$overlay_root/bin" 2>/dev/null; then
    printf 'bare `cd -` reintroduced; use absolute cd "$HOME" or mktemp+trap\n' >&2
    return 1
  fi
}

@test "no echo \"\$PWD\" in overlay shell (P2-10 regression guard)" {
  local overlay_root
  overlay_root="$(cd -- "$BATS_TEST_DIRNAME/.." && pwd)"
  if grep -REn 'echo[[:space:]]+"?\$PWD"?' \
       "$overlay_root/install.sh" \
       "$overlay_root/wrappers" \
       "$overlay_root/lib" \
       "$overlay_root/scripts" \
       "$overlay_root/bin" 2>/dev/null; then
    printf 'echo "$PWD" reintroduced; use printf %%s "$PWD" instead\n' >&2
    return 1
  fi
}
