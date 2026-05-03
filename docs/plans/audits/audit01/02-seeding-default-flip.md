# Phase 02: Seeding default — flip to off, align docs

> Maps to audit findings: P0-3

## Goal
Make Claude session-history seeding **opt-in** (the documented behavior in
the wrapper header). Today `claude-amq:26-27` is opt-out via
`CLAUDE_AMQ_NO_SEED`, while `:10-17` documents opt-in via
`CLAUDE_AMQ_SEED_FROM_PARENT`. Default-on rsyncs the parent worktree's chat
history into every fresh pane: disk amplification, billing escalation,
cross-worktree info leak.

## Pre-conditions
- Phase 00 scaffolding present.

## Files to touch
- `dux-amq/wrappers/claude-amq` — flip the guard.
- `dux-amq/README.md` — align "Trade-offs" with new default.
- `dux-amq/tests/seed_default_off.bats` — regression test.

## Steps
1. Flip the guard:
   ```diff
   - # On by default. Set CLAUDE_AMQ_NO_SEED=1 to skip.
   - [[ "${CLAUDE_AMQ_NO_SEED:-}" == "1" ]] && return 0
   + # Off by default. Set CLAUDE_AMQ_SEED_FROM_PARENT=1 to enable.
   + [[ "${CLAUDE_AMQ_SEED_FROM_PARENT:-}" == "1" ]] || return 0
   ```
2. Keep the existing one-line stderr notice when seeding fires; it is the
   user's only visible signal that history was copied.
3. Add a sourcing guard so the wrapper is testable without `exec claude`:
   ```diff
   + # Allow tests to source this file without launching claude.
   + (return 0 2>/dev/null) && return 0
   ```
   immediately before the `ME=…` block. (Exits 0 when sourced, falls through when executed.)
4. bats regression test:
   ```bash
   # dux-amq/tests/seed_default_off.bats
   load 'lib/setup'
   setup() { setup_isolated_home; }
   teardown() { teardown_isolated_home; }

   @test "seeding off by default" {
     mkdir -p "$HOME/.claude/projects/-tmp-parent"
     touch    "$HOME/.claude/projects/-tmp-parent/abc.jsonl"
     run bash -c 'source dux-amq/wrappers/claude-amq; seed_session_history; ls $HOME/.claude/projects/'
     [ "$status" -eq 0 ]; [[ "$output" != *"-tmp-self"* ]]
   }
   @test "seeding fires when CLAUDE_AMQ_SEED_FROM_PARENT=1" {
     export CLAUDE_AMQ_SEED_FROM_PARENT=1
     # fixture as above; assert copy happened.
   }
   ```
5. README "Trade-offs": rewrite to "off by default — set
   `CLAUDE_AMQ_SEED_FROM_PARENT=1` to copy parent history; pair with
   `resume_args = ["--resume"]` in `config.toml`."

## Validation
- `bats dux-amq/tests/seed_default_off.bats` passes both cases.
- Manual: fresh worktree, no env var → no jsonls in
  `~/.claude/projects/<encoded-self>/`. Set the var, re-open → stderr
  prints seeded count; jsonls present.
- `grep -nR CLAUDE_AMQ_NO_SEED dux-amq/` empty.

## Acceptance criteria
- [x] Only the `CLAUDE_AMQ_SEED_FROM_PARENT=1` opt-in path remains.
- [x] No `CLAUDE_AMQ_NO_SEED` reference in wrapper/config/scripts/install.sh. (README's "Migrating from earlier versions" bullet intentionally names the old var so upgraders see the hint.)
- [x] README "Trade-offs" matches new default + migration steps.
- [x] bats covers default-off and explicit-opt-in (`tests/seed_default_off.bats`, 4 cases).
- [x] Sourcing the wrapper does not exec claude (`(return 0 …) && return 0` guard before `seed_session_history || true`).

## References
- Audit P0-3 (recommendation lines 56–62).
- Anthropic Claude Code session storage docs (sessions written under `~/.claude/projects/<encoded-cwd>/`; see Phase 04 for encoder details).
