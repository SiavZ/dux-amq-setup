# Architecture, maintainability, and modernity

This pass looked for unnecessary dependencies/abstractions, obsolete compatibility layers, unsafe duplication, large-module ownership problems, missing reuse, and mechanical simplifications. Baseline is `3d52074`. Under the frozen plan, aesthetic refactors are not fixable P2s unless deletion is safe, replacement is mechanical, or tests prove the change.

## P2-01 — Terminal widths use Unicode scalar counts despite a width-aware renderer

**Impact.** CJK, emoji, combining marks, and zero-width characters can mis-size buttons and wrap checkbox labels at the wrong terminal column. This is a localized correctness/simplification issue, not a crash (P0-08 is the separate unsafe slice).

**Baseline evidence.** `3d52074 — src/app/components/button.rs:27-35 — button_width_for` says `chars().count()` measures visible width, which is false for terminal cells. Its test at lines 239-243 calls each CJK character one visible column even though `世` is normally double-width. `3d52074 — src/app/components/checkbox.rs:72-100,175-239 — layout/wrap_checkbox_label/push_broken_word_lines` uses the same scalar count for indentation, words, and chunks. The project already calls ratatui `Line::width()` at `src/diff.rs:432` and `src/app/render.rs:927,1687`; locked ratatui 0.30.0 already resolves `unicode-width 0.2.2` (`Cargo.lock:1868-1870,2819-2821`).

**Adversarial verification.** Byte length would be worse, but scalar count is not display width. Minimum width masks short labels, so a long mixed CJK label is the direct button reproduction; checkbox wrapping reproduces at any tight width. No new dependency or abstraction is needed: reuse the renderer's width calculation and make the existing tests assert terminal columns. This is a mechanical, test-proven P2.

**Fixability proof.** Replace width calculations with the existing ratatui display-width API, preserve saturating conversion/minimums, and change/add CJK, emoji, and combining-mark expectations. State: confirmed/fixable.

## P2-02 — A private fake re-export and no-op function are safely deletable

**Impact.** Dead scaffolding claims to expose a public test surface but exposes nothing, obscures import ownership, and deliberately suppresses useful compiler diagnostics.

**Baseline evidence.** `3d52074 — src/cli.rs:270-279 — _PurgeReportPubReexport / _purge_used_re_exports`: a private type alias is described as a re-export, is marked dead-code-allowed, and a private never-called function only touches `PurgeOutcome::Done`. Full-file symbol search shows `PurgeReport` appears only in the import and alias, and `PurgeOutcome` only in the no-op. External users cannot name a private alias.

**Adversarial verification.** No macro expansion, test-only module, or public `pub use` references either symbol. Removing the alias/function and unused `PurgeReport` import changes no reachable code; `purge::PurgeOutcome` is not imported. Compiler/test proof is straightforward. This meets the plan's safe-deletion P2 bar.

**Fixability proof.** Delete lines 270-279 and remove `PurgeReport` from the import; normal compile/test is the proof. State: confirmed/fixable.

## Documented observations — not fixable findings

### Very large ownership modules

Baseline line counts include:

| File | Lines |
|---|---:|
| `src/app/input.rs` | 12,367 |
| `src/app/render.rs` | 7,239 |
| `src/config.rs` | 4,159 |
| `src/app/mod.rs` | 4,015 |
| `src/app/sessions.rs` | 3,780 |
| `src/keybindings.rs` | 2,798 |
| `src/app/workers.rs` | 2,058 |
| all Rust under `src/` | 58,209 |

This conflicts in spirit with `CLAUDE.md:135-136`'s guidance to extract feature areas beyond roughly 200 lines and increases review/merge/test-selection cost. It is **not a fixable audit03 finding**: no safe deletion or mechanical boundary was proved, and splitting thousands of lines during a security/data-loss fix phase would create risk without behavior proof. Later work should extract only when a concrete feature already has an ownership seam; no “architecture cleanup” epic is warranted.

### Repeated wrapper/bootstrap logic

Claude, Codex, and Gemini wrappers repeat handle normalization, identity registration, wake daemon setup, root defaults, and collision handling. A shared sourced shell library could reduce bug multiplicity, but sourcing/path/install semantics create their own failure modes. No extraction was proved smaller or safer than fixing the three copies and strengthening shared Bats cases. Documented, not fixable.

### Historical phase comments and dead-code allowances

There are many `audit01`/`audit02`/phase/follow-up comments and 83 matches for TODO/FIXME/dead-code/follow-up/audit03 markers across `src/`. Some are valuable rationale; others describe work as deferred after the feature shipped. A blanket removal cannot distinguish durable invariant from stale narrative mechanically. Apart from P2-02's proven no-op, this is not a fixable finding.

### Dependency/toolchain posture

The crate uses edition 2024 and pins Rust 1.88.0 with an explicit minimum-version rationale (`Cargo.toml:1-4`; `rust-toolchain.toml:1-13`). Core UI/SQLite/logging dependencies are contemporary at the baseline. Broad “update everything” advice would be churn, not an audit result. Only exact advisory/tool reproducibility changes in [05-ci-release-supply-chain.md](05-ci-release-supply-chain.md) are justified now.

### Existing architectural strengths

- Provider execution remains command/config driven rather than acquiring a custom RPC/adaptor layer.
- `app/state/` separates Git, runtime, and UI state; typed `SessionState` narrows lifecycle transitions.
- Background worker channels, raw-byte Git/diff helpers, atomic log redaction, rate/cap primitives, and a large unit/Bats/integration corpus provide reusable rails.
- The smallest fixes for most audit03 findings are local invariant repairs. The evidence does not support replacing ratatui, SQLite, portable-pty, the shell wrappers, or AMQ.

## Architecture conclusion

Only two architecture items meet P2's fixability threshold. The most valuable maintainability work is to make existing contracts truthful and reuse already-present primitives—display width, worker events, path containment, temporary-file rename, SQLite transactions—rather than add layers.
