# Phase 12: Versioned config inserts — `>>> dux-amq vN.M.K >>>`

> Maps to audit findings: P1-7

## Goal
Make `~/.bashrc` and `~/.claude/CLAUDE.md` reapplies actually update on
re-run. Today idempotency is gated on the literal markers `=== dux + AMQ ===`
and `Multi-agent environment (AMQ + dux)` — if `bashrc-additions.sh` or
`claude-md-additions.md` change, re-running `install.sh` is a no-op and the
user keeps the stale block. Switch to versioned begin/end markers and a
delete-and-rewrite pattern (the `pyenv`/`sdkman` shape).

## Pre-conditions
- Phase 00 baseline.

## Files to touch
- `dux-amq/VERSION` — single-line semver of the overlay (e.g. `0.1.0`).
- `dux-amq/install.sh` — bashrc + CLAUDE.md insert blocks.
- `dux-amq/config/bashrc-additions.sh` + `dux-amq/config/claude-md-additions.md` — wrap in versioned markers.
- `dux-amq/tests/idempotent_inserts.bats` — create.

## Steps
1. Marker convention. bash file:
   ```
   # >>> dux-amq vN.M.K >>>
   <content>
   # <<< dux-amq vN.M.K <<<
   ```
   Markdown file: use `<!-- … -->` so `#` doesn't render as a heading.
2. Read the version: `DUX_AMQ_VERSION=$(< "$HERE/VERSION")`.
3. Helper using portable `awk` with regex-matched markers (strips any
   prior version's block automatically):
   ```bash
   strip_block() {
     local file="$1"; [[ -f "$file" ]] || return 0
     awk '/^# >>> dux-amq v[^ ]+ >>>$/{s=1} /^# <<< dux-amq v[^ ]+ <<<$/{s=0;next} !s' \
       "$file" > "$file.tmp" && mv "$file.tmp" "$file"
   }
   ```
   For the markdown file, use `<!-- >>> dux-amq v…` patterns.
4. Wrap the additions files. `bashrc-additions.sh`:
   ```diff
   + # >>> dux-amq vREPLACE_AT_INSTALL >>>
   # === dux + AMQ ===
   …
   # === end dux + AMQ ===
   + # <<< dux-amq vREPLACE_AT_INSTALL <<<
   ```
   At install time, `sed "s|REPLACE_AT_INSTALL|$DUX_AMQ_VERSION|g"` while
   appending.
5. Replace the `grep -q` gate with strip-and-rewrite:
   ```diff
   - if ! grep -q '=== dux + AMQ ===' "$HOME/.bashrc"; then
   -   cat "$HERE/config/bashrc-additions.sh" >> "$HOME/.bashrc"
   - fi
   + strip_block "$HOME/.bashrc"
   + sed "s|REPLACE_AT_INSTALL|$DUX_AMQ_VERSION|g" "$HERE/config/bashrc-additions.sh" >> "$HOME/.bashrc"
   ```
   Same for `~/.claude/CLAUDE.md` with `<!--` markers.
6. Legacy migration: if file contains the old `=== dux + AMQ ===` marker
   but no `>>> dux-amq v` marker, strip the legacy block first.
7. bats: first-run, upgrade (bump VERSION → old block gone, new in place,
   no duplicate), same-version-rerun (file unchanged), and legacy migration.

## Validation
- `bats dux-amq/tests/idempotent_inserts.bats` covers all cases.
- Manual: `tail -50 ~/.bashrc` after two installs at different versions →
  exactly one `>>> … >>>` / `<<< … <<<` pair.

## Acceptance criteria
- [x] `dux-amq/VERSION` exists and is read at install time. *(Track A scope: inlined as `DUX_AMQ_VERSION="0.1.0"` in `install.sh` with a `# AUDIT01-VERSION` comment so Phase 15's release pipeline can mechanically rewrite it. Promoting to a separate `VERSION` file is a Phase 15 follow-up.)*
- [x] All inserts wrapped in versioned begin/end markers (bashrc: `# >>> dux-amq vN.M.K >>>` / `<<<`; CLAUDE.md: `<!-- >>> dux-amq vN.M.K >>> -->` / `<<<`).
- [x] `strip_block` removes any prior versioned block (regex via `awk` matching `v[^ ]+`).
- [x] Legacy unversioned block is migrated cleanly on upgrade (both bashrc `# === dux + AMQ ===` and CLAUDE.md `## Multi-agent environment (AMQ + dux)` sections).
- [ ] bats covers first-run, upgrade, same-version, legacy. *(Deferred — Track A cannot create new test files. Validation was performed via the manual repro in this branch's commit message; Track B should land the bats fixtures.)*

## References
- Audit P1-7.
- conda init idempotency issue (cautionary tale): https://github.com/conda/conda/issues/8703
- pyenv shell init markers (canonical pattern): https://github.com/pyenv/pyenv
