# Phase 04: Path encoding parity + realpath containment check

> Maps to audit findings: P0-5

## Goal
Replace the hand-rolled `sed` path-encoder in `claude-amq:38-39` and the
prefix-glob CWD check in `:67`. The encoder mismatches Claude Code's
on-disk encoding for paths with `-`, repeated `_`, or unicode (silent
seeding no-op or wrong session); the glob `"$DUX_HOME/worktrees/"*`
matches `…/worktrees-evil/x` because the boundary `/` is not enforced.

## Pre-conditions
- Phase 00 scaffolding.
- A locally installed Claude Code build to capture observed encoder
  behavior, OR a fixture tsv reverse-engineered from the on-disk dirs.

## Files to touch
- `dux-amq/lib/path-encode.sh` — canonical encoder, sourced by all wrappers.
- `dux-amq/wrappers/{claude,codex,gemini}-amq` — switch to the lib + new check.
- `dux-amq/tests/path_encode.bats` + `dux-amq/tests/fixtures/path-encoding.tsv`.

## Steps
1. **Verification needed before implementation**: empirically determine
   Claude Code's encoder. Run claude in throwaway dirs `/tmp/a-b`,
   `/tmp/a_b`, `/tmp/Æneid`; record `~/.claude/projects/` results in
   `path-encoding.tsv` (`<absolute>\t<encoded>`). Prefer
   `claude config sessions-dir` if it exists — public-API path.
2. Sourced lib (use `printf`, not `echo`):
   ```bash
   # dux-amq/lib/path-encode.sh
   path_encode() {
     local p="$1"
     printf '%s' "${p//\//-}"  # adjust per fixture
   }
   ```
3. Replace inline encoder in `claude-amq`:
   ```diff
   - enc_self=$(echo "$PWD"           | sed 's|/|-|g; s|_|-|g')
   - enc_main=$(echo "$main_worktree" | sed 's|/|-|g; s|_|-|g')
   + source "$DUX_AMQ_LIB/path-encode.sh"
   + enc_self=$(path_encode "$PWD")
   + enc_main=$(path_encode "$main_worktree")
   ```
   `$DUX_AMQ_LIB` is set in `bashrc-additions.sh` to a deterministic
   absolute install path (see Phase 12). Fail loudly if missing.
4. Replace the prefix glob with realpath-canonicalised containment, in
   all three wrappers:
   ```diff
   - if [[ -z "$ME" && "$PWD" == "${DUX_HOME:-/data/state/dux}/worktrees/"* ]]; then
   + DUX_WTS=$(realpath -m "${DUX_HOME:-/data/state/dux}/worktrees")
   + PWD_REAL=$(realpath -m "$PWD")
   + if [[ -z "$ME" && "$PWD_REAL" == "$DUX_WTS"/*/* ]] \
   +   || [[ -z "$ME" && "$PWD_REAL" == "$DUX_WTS"/* && -d "$PWD_REAL" ]]; then
       ME=$(basename "$PWD_REAL")
     fi
   ```
   The boundary `/` rejects `…/worktrees-evil/x`.
5. bats fixture test (`tests/path_encode.bats`): one test reads each
   `<path>\t<expected>` row from `path-encoding.tsv` and asserts
   `path_encode "$path" == "$expected"`. A second test creates
   `/tmp/dh/worktrees-evil/x`, sets `DUX_HOME=/tmp/dh`, sources
   `claude-amq` from inside it, and asserts `$ME` is **not** `"x"` — the
   sibling-prefix glob is now rejected.

## Validation
- `bats dux-amq/tests/path_encode.bats` green.
- Manual: worktree path with hyphen + unicode → `~/.claude/projects/<enc>`
  matches what Claude Code itself created (no double dirs).
- `shellcheck dux-amq/wrappers/* dux-amq/lib/path-encode.sh` clean.

## Acceptance criteria
- [x] One shared `path_encode` in `dux-amq/lib/path-encode.sh`, sourced by `claude-amq` (codex/gemini don't seed so don't need the encoder; they DO carry the realpath check).
- [x] Fixture covers 12 representative paths (hyphen, underscore, dot, space, unicode codepoint, multi-byte unicode) — `dux-amq/tests/fixtures/path-encoding.tsv`, captured against claude 2.1.111 on 2026-05-03.
- [x] Realpath containment check replaces the glob in all three wrappers.
- [x] Negative test for `worktrees-evil/` sibling passes (`path_encode.bats` test 3).
- [ ] Install copies `path-encode.sh` to `$DUX_AMQ_LIB` — **deferred to Phase 12** (`install.sh` is owned by Track A install-chain). Track B's wrappers locate the lib via `$DUX_AMQ_LIB` env var, `../lib` relative to the resolved script path, or canonical install locations; fail loudly with a red banner if absent.

### Verification gap closed

The plan suggested `s|/|-|g; s|_|-|g` as a starting encoder. Empirical
testing showed Claude Code actually replaces every Unicode CODEPOINT
not in `[A-Za-z0-9-]` with a single `-` (runs preserved, case
preserved). Dots, spaces, `+`, `~`, `@`, multi-byte unicode are all
mapped to `-` — none of which the old `sed` handled. `lib/path-encode.sh`
implements the codepoint-correct rule via python3 with a byte-wise
`tr -c` fallback for ASCII-only paths.

## References
- Audit P0-5.
- `realpath -m`: https://man7.org/linux/man-pages/man1/realpath.1.html
- Verification gap: encoder is observed empirically; switch to a public CLI surface if Anthropic ships one.
