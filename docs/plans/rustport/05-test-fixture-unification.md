# Phase 05 — Test fixture unification and inline-test migration

**Track:** B (safety net) · **Parallel-safe with:** 06, 07
**Depends on:** nothing · **Blocks:** 16, 17, 21 — and practically all of Track D

## Goal

Remove the two mechanical obstacles that make every later split expensive: two
hand-rolled `App` literals that break on any field move, and ~12,500 lines of inline
tests reaching private items via `use super::*`.

## Evidence

- **Two hand-built `App` fixtures**, each enumerating all 48 `App` fields plus full
  `UiState` / `RuntimeState` / `GitState` literals: `input.rs:6238` (`test_app`) and
  `sessions.rs:2847` (`test_app_with_sessions`), constructed at `input.rs:6374` and
  `sessions.rs:2955`. **Any field move breaks both.**
- The production `App { .. }` literal is **137 lines** at `mod.rs:1598-1647`, which is
  what makes `bootstrap_with_lock` unsplittable.
- Inline tests are **27–39%** of the files being split:

| File | Total | Prod | Test | #tests |
|---|---|---|---|---|
| `input.rs` | 13,266 | 6,163 | **7,102** | 236 |
| `sessions.rs` | 4,713 | 2,820 | **1,893** | 53 |
| `workers.rs` | 4,394 | 3,306 | **1,088** | 16 |
| `text_input.rs` | 1,804 | 768 | **1,036** | 93 |
| `config.rs` | 4,952 | 3,237 | **1,714** | — |
| `cli.rs` | 1,991 | 1,206 | **785** | — |
| `keybindings.rs` | 2,862 | 2,101 | **760** | — |
| `render.rs` | 7,565 | 7,178 | 386 | 43 |
| `pty.rs` | 2,161 | 1,357 | **803** | — |
| `git.rs` | 2,236 | 1,282 | **954** | — |
| `peer.rs` | 2,281 | 1,332 | **949** | — |

  The 500-line arithmetic **does not work** without moving these.
- **No file under `tests/` covers `src/app/`** — `dux::app` appears zero times across
  all 13 integration test files, even though `lib.rs:14` declares `pub mod app` and
  `App` is public. Nor does any `tests/` file cover keybindings, themes, or CLI parsing.

## In scope

One shared `App::for_test()` builder, migration of inline tests to sibling files, and
promotion of a few genuinely integration-level tests into `tests/`.

## Out of scope

- Splitting production code → Track D.
- New behavioural tests for render → Phase 06.
- New config property tests → Phase 07.
- Changing what any test asserts. **This phase moves tests; it does not rewrite them.**

## Work items

1. **Build `App::for_test()`** as a builder with sane defaults, replacing both
   `input.rs:6238` and `sessions.rs:2847`. It must cover the union of what both
   fixtures provide (sessions, projects, PTY-less state, a `RuntimeBindings` from
   defaults) and expose `with_sessions(..)`, `with_projects(..)`, `with_prompt(..)`
   chainable setters. Place it under `#[cfg(test)]` in `src/app/test_support.rs`, or
   behind a `test-support` feature if Phase 06's integration tests need it from
   `tests/`.
2. **Convert both existing fixtures to delegate** to the builder, in a commit that
   changes no assertions. All 289 tests in those two files must stay green.
3. **Migrate inline tests to sibling files** using
   `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`, which preserves `use super::*`
   access to private items. **One commit per file**, no assertion changes. Order by
   payoff: `input.rs` (7,102) → `sessions.rs` (1,893) → `config.rs` (1,714) →
   `workers.rs` (1,088) → `text_input.rs` (1,036) → `git.rs` (954) → `peer.rs` (949) →
   `pty.rs` (803) → `cli.rs` (785) → `keybindings.rs` (760) → `render.rs` (386).
4. **Where a test file itself exceeds 500 lines after the move, split it by concern**,
   not arbitrarily — mirroring the production module tree the corresponding Track D
   phase will create, so the eventual split is a rename rather than a re-partition.
5. **Promote genuinely integration-level tests into `tests/`.** Specifically: the
   `TestBackend`-driven tests at the 7 existing sites in `input.rs`, and the
   `close_top_overlay` coverage currently incidental at `input.rs:9915`. This starts
   closing the "zero `tests/` coverage of `src/app/`" gap.
6. **Add `serial_test` annotations where the move exposes shared-state coupling.**
   Some inline tests currently pass only because of in-file ordering; moving them can
   surface real flakiness. Fix by isolating state, not by re-serialising, unless the
   state is genuinely process-global.
7. **Record a per-file test-count baseline** before and after each move and assert
   equality. This is the only mechanical defence against a dropped test during a
   12,500-line migration.

## Acceptance criteria

- [ ] Exactly one `App` test fixture exists; `grep -c 'App {' src/app/**/*_tests.rs`
      shows no hand-rolled full literals.
- [ ] All eleven files above have their inline tests in sibling `*_tests.rs` files.
- [ ] `cargo test --all-features` reports **the same distinct test count as the
      baseline** (1,139) — no test silently lost.
- [ ] Per-file before/after test counts recorded in the PR description and equal.
- [ ] No `*_tests.rs` file exceeds 500 lines, or carries a justified
      `// allow-long-file:` directive.
- [ ] `tests/` contains at least one file exercising `dux::app` (currently zero).
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` still exits 0.
- [ ] No assertion text changed in any migrated test (verify with
      `git diff --stat` showing pure moves plus the `#[path]` declarations).

## Validation

```bash
# baseline, captured before starting
cargo test --all-features 2>&1 | tee /tmp/before.txt

# after each file's migration
cargo test --all-features 2>&1 | tee /tmp/after.txt
diff <(grep -oE 'test [a-z_:]+ \.\.\. ok' /tmp/before.txt | sort) \
     <(grep -oE 'test [a-z_:]+ \.\.\. ok' /tmp/after.txt  | sort)
# expect: only module-path renames, no removals

cargo clippy --all-targets --all-features -- -D warnings
ci/check-file-length.sh
```

## Risks

| Risk | Mitigation |
|---|---|
| A test is silently dropped in a 7,102-line move | The name-level diff above is mandatory per commit, not per phase |
| `use super::*` breaks for items the sibling file cannot see | `#[path]` keeps the test module *inside* the parent module, so private access is preserved — this is why `#[path]` is specified rather than a `tests/` move |
| Moving tests surfaces latent ordering flakiness | Treat as a real bug found, not an obstacle: fix the shared state. Only reach for `serial_test` when the state is process-global |
| The builder's defaults differ subtly from the old fixtures | Convert the fixtures to delegate **first** (work item 2) and confirm green before any file moves |

## References

- `artifacts/research-app-monoliths.md` §Risk register (breakage vectors 1–3), R10
- `artifacts/research-config-keys-theme-cli.md` §R8 (test placement blocks the split)
- `CONVENTIONS.md` §3
