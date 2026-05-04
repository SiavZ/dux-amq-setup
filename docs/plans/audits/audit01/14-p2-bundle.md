# Phase 14: P2 bundle — clipboard, license, config, bash hygiene

> Maps to audit findings: P2-1, P2-2, P2-4, P2-5, P2-8, P2-9, P2-10

## Goal
Land seven small P2 fixes as one PR `chore(audit01): P2 polish bundle`.

## Pre-conditions
- Phase 06 has extracted `patches/0001-clipboard-osc52.diff`.
- Phase 00 baseline test green.

## Files to touch
- `src/clipboard.rs` (P2-1, P2-2); `tests/clipboard_st_fallback.rs` (test).
- `dux-amq/README.md` (P2-4); `dux-amq/LICENSE` (P2-5).
- `dux-amq/install.sh` (P2-8, P2-9, P2-10); `dux-amq/wrappers/claude-amq` (P2-10).
- `patches/0001-clipboard-osc52.diff` — regenerate.

## Steps
1. **P2-1: OSC 52 ST fallback.** Some terminals reject BEL terminator.
   Env-gated switch:
   ```diff
   fn osc52_sequence(text: &str) -> String {
   -    format!("\x1b]52;c;{}\x07", base64_encode(text.as_bytes()))
   +    let term = if std::env::var("DUX_OSC52_TERMINATOR").as_deref() == Ok("ST")
   +        { "\x1b\\" } else { "\x07" };
   +    format!("\x1b]52;c;{}{}", base64_encode(text.as_bytes()), term)
   }
   ```
   Document `DUX_OSC52_TERMINATOR=ST` in README. Test asserts the sequence
   ends in `\x1b\\` when set, `\x07` when unset.
2. **P2-2: `Clipboard::new` returns `Result`.** Replace `.expect` with
   `.context(…)?` and switch the return type. In `App::new`, on `Err` log
   and fall back to a no-op sender (introduce a small `ClipboardLike`
   trait to keep the call site clean).
3. **P2-4: Data-handling README section** (sibling to Phase 05). List
   dirs with chat/PII: `~/.claude/projects/`, `/data/state/{agents,codex,gemini,amq,dux}`.
   Note GDPR categories; cross-link Phase 05's LUKS recipe.
4. **P2-5: Overlay copyright.** New `dux-amq/LICENSE` (MIT, `Copyright (c)
   2026 SiavZ (dux-amq overlay)`); repo-root LICENSE unchanged. README
   notes dual-license attribution.
5. **P2-8: Preserve hand-edited `config.toml`.** Detect non-default
   content; require `FORCE_REGEN=1` to overwrite:
   ```diff
   + if [[ -f "$STATE_ROOT/dux/config.toml" ]] \
   +    && grep -qE '^projects\s*=|^\[macros\.' "$STATE_ROOT/dux/config.toml" \
   +    && [[ "${FORCE_REGEN:-}" != "1" ]]; then
   +   warn "config.toml has user content; skipping regenerate (FORCE_REGEN=1 to overwrite)"
   +   SKIP_REGEN=1
   + fi
   + [[ "${SKIP_REGEN:-}" != "1" ]] && DUX_HOME="$STATE_ROOT/dux" dux config regenerate --yes >/dev/null
   ```
   Always run the `sed` patch block (idempotent against unique upstream defaults).
6. **P2-9: Preserve mode on jq settings merge.**
   ```diff
   - jq … > "$f.tmp" && mv "$f.tmp" "$f"
   + jq … > "$f.tmp" || { warn "jq merge failed"; rm -f "$f.tmp"; return 1; }
   + install -m "$(stat -c '%a' "$f")" "$f.tmp" "$f" && rm -f "$f.tmp"
   ```
7. **P2-10: bash quoting.** Confirm `cd -` (replaced by Phase 01's
   `mktemp + trap`) and `echo "$PWD"` (replaced by Phase 04's `printf`)
   are gone. `shellcheck` over the overlay must be green after this phase.

## Validation
- `cargo test --test clipboard_st_fallback` passes.
- `cargo clippy --all-targets -- -D warnings` clean.
- `shellcheck` green across overlay.
- README renders Data-handling + license attribution.
- Manual: `dux config regenerate` skips on hand-edited file; runs cleanly
  on fresh install.

## Acceptance criteria
- [ ] `DUX_OSC52_TERMINATOR=ST` switches terminator; default stays BEL.
- [ ] `Clipboard::new` → `Result`; App falls back to no-op on error.
- [ ] README has both Security-model (Phase 05) and Data-handling sections.
- [ ] `dux-amq/LICENSE` exists with overlay-author copyright.
- [ ] `regenerate` gated on user content; `FORCE_REGEN=1` overrides.
- [ ] jq merge preserves mode.
- [ ] No `cd -` or `echo "$PWD"` remains; `shellcheck` green.

## References
- Audit P2-1, P2-2, P2-4, P2-5, P2-8, P2-9, P2-10.
- OSC 52 terminators: xterm ctlseqs; WezTerm clipboard docs.
