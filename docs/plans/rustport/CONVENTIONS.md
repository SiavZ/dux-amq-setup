# Conventions — binding on every rustport phase

Referenced by all 24 phase files. Changes here apply everywhere; do not restate
these rules inside a phase file.

## 1. File-header comment block

**Every** `.rs` file opens with a `//!` module doc comment containing: a one-line
purpose, a short paragraph of behavioural contract, a `# Uses` tree, and — for any
module that is depended upon — a `# Used by` tree.

```rust
//! Renders the diff overlay pane.
//!
//! Owns the syntect-highlighted diff view and its scroll state. Pure: every call
//! must be cheap enough to run inside one frame. No git I/O here — the caller
//! supplies an already-computed `FileDiff`.
//!
//! # Uses
//!
//! ```text
//! app::render::overlays::diff
//! ├── crate::theme            (semantic colours; never raw Color::*)
//! ├── crate::model::FileDiff
//! └── app::components
//!     ├── scrollbar
//!     └── button
//! ```
//!
//! # Used by
//!
//! ```text
//! app::render::overlays::diff
//! ├── app::render::overlays    (modal dispatch)
//! └── app::render::panes::center
//! ```
```

### The fence rule — non-negotiable

The tree **must** be fenced as `` ```text ``.

- An **unannotated** `` ``` `` fence inside a doc comment **is a Rust doctest** —
  rustdoc treats it as `` ```rust ``. A tree containing `├──` will not parse as
  Rust and `cargo test` fails.
- A **4-space-indented block is also a doctest, and cannot be escaped** — there is
  no way to apply `ignore` to an indented code block
  ([rust#100225](https://github.com/rust-lang/rust/issues/100225),
  [rust#94757](https://github.com/rust-lang/rust/issues/94757)). **Never indent the
  tree.**
- `no_run` does **not** help — it still compiles. `ignore` works but still
  syntax-highlights as Rust and renders a "not tested" marker. **Use `text`.**
- Safety net: `rustdoc::invalid_rust_codeblocks` is warn-by-default and fires on
  doc code blocks that are not parsable as Rust, so a forgotten annotation surfaces
  at `cargo doc` time.
- Caveat: doctests are extracted from the **library target only**. `dux` has
  `src/lib.rs` (`lib.rs:14` declares `pub mod app`), so doctests do run — but the
  new `dux-amq-rust` crate must keep a lib target for the same protection.

`//!` (inner) is correct, not `//`. Presence is enforced by
`clippy::missing_docs_in_private_items`; **accuracy of the tree is enforced by the
generator in Phase 02**, because no lint can verify it.

## 2. The 500-line rule

No `.rs` file exceeds **500 lines**, tests included.

- There is **no Rust lint for file length**. `clippy::too_many_lines` is
  functions-only (default 100). A `too_many_lines_in_file` lint was proposed
  ([PR #16675](https://github.com/rust-lang/rust-clippy/pull/16675)) but is still
  open with merge conflicts and no approval — **do not plan around it landing**.
- The gate is a custom CI script (Phase 02), following the precedent of rustc's own
  `src/tools/tidy/src/style.rs`, which hardcodes a limit and supports a per-file
  opt-out directive.
- **Opt-out:** `// allow-long-file: <reason>` within the first 5 lines. Requires a
  concrete reason. Generated files and vendored fixtures qualify; "it's hard to
  split" does not.

### How to split — the house rule

Prefer **extracting a reusable module** over mechanically cutting a file in half.
If two call sites would use the extracted code, it is a module; if one would, it is
probably just a smaller file. The reusable extractions already identified are
enumerated per phase (R1–R13 in `artifacts/research-app-monoliths.md`, plus the
config/keys sets) — prefer those over inventing new seams.

Rust permits **many `impl App` blocks across sibling modules** — the codebase
already does this in 9 files. A directory split therefore needs no trait objects,
no newtypes, and no dyn dispatch.

## 3. Test placement

Inline `#[cfg(test)]` modules count toward the 500-line limit. They are **27–39%**
of the files being split, so the arithmetic does not work without moving them.

- Move inline tests to a sibling file included with
  `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;`. This preserves `use super::*`
  access to private items, which is what most existing tests rely on.
- **The test migration is always its own commit, landed before the production
  split.** Mixing them makes the diff unreviewable and hides dropped tests.
- Inline tests travel with their code, so a pure file split preserves them for
  free — but only if the move is mechanical. Any test that changes during a split
  must be called out in the PR description.

## 4. Colours, keybindings, git, strings

These are existing project tenets (`CLAUDE.md`); the research confirmed which are
already honoured, so do not spend budget re-auditing them:

- **Theme — verified clean.** Zero real raw `Color::*` violations in production
  render code (the naive grep's 100 hits are `Color::Reset` sentinels and the
  xterm256/grayscale converters). Keep it that way; use `theme.rs` semantic names.
- **Git safety — verified clean.** `--porcelain=v1 -z`, `--numstat -z`,
  `symbolic-ref --quiet --short HEAD`, `-c color.diff=false` are all in use.
- **Byte-slicing — verified clean.** No panic risk found; `char_indices().nth()`
  and `TextInput`'s enforced boundary invariant are used correctly.
- **Config documentation — verified clean.** 53/53 keys documented; `comment: None`
  appears zero times.
- **Keybinding labels — 5 violations exist** and are fixed in Phase 10. Never add a
  sixth: look the label up via `RuntimeBindings::label_for()`. The reference
  implementation is `WELCOME_TIPS` (`app/render.rs:58-220`), where every tip is
  `fn(&RuntimeBindings) -> String`.

## 5. Behaviour preservation

- **Any change to a user-visible string is a breaking change** until proven
  otherwise. The bats suite pins dozens verbatim
  (`artifacts/bats-test-parity-matrix.md` §4.3).
- **Status-line messages stay verbose and actionable.** "Changes committed
  successfully. Press ^U to push to remote." — not "Committed."
- A refactor that changes behaviour is a bug, not an improvement. If a phase
  discovers that current behaviour is wrong, it fixes it **as an explicit numbered
  work item with its own test**, never as a silent side effect of moving code.

## 6. Commit discipline

- One phase may span several commits, but each commit must leave `cargo fmt`,
  `cargo clippy -D warnings`, and `cargo test` green.
- Test migration, mechanical move, and behavioural change are **always separate
  commits**.
- Every commit touching attack surface updates `SECURITY.md` and
  `docs/operations/threat-model.md` in the same commit (project rule; new threat
  IDs append as `T13`, `T14`, … and never reshuffle existing rows).

## 7. Validation — run before every push

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
ci/check-file-length.sh                       # Phase 02
cargo modules orphans --deny --bin dux        # Phase 02
bats dux-amq/tests                            # overlay suite
```

The clippy invocation is a CI gate and must pass with zero warnings. A new stable
release can enable lints that previously passed; **fix the code rather than
suppressing the lint** unless there is a specific, documented reason.
