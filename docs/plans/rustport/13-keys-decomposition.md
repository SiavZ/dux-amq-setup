# Phase 13 — `src/keybindings.rs` decomposition

**Track:** D (decomposition) · **Parallel-safe with:** 12, 14, 15, 16, 17
**Depends on:** 02, 05, 07 · **Blocks:** 24

## Goal

Split 2,101 production lines (2,862 with tests) into `src/keys/`, including the
1,090-line `BINDING_DEFS` table — the only structurally non-trivial part — without
changing a single binding resolution.

## Why this is delicate

`BINDING_DEFS` (`keybindings.rs:498`) is one declarative table that drives **everything**:
default keys, scopes, help text, hint contexts, and palette entries for 80 `Action`
variants. It is consumed from outside the module too — `config.rs`'s `validate_keys`
(`:3049`) and `render_keys_config` both iterate it. Two properties are load-bearing:

- **Declaration order is the tiebreak** for conflicting bindings, documented at
  `keybindings.rs:472-474` and tested by `lookup_declaration_order_wins` (`:2612-2638`).
  Splitting the table across files **changes concatenation order** unless done carefully.
- **Same-key-different-scope reuse is deliberate.** `ctrl-g` serves `ExitInteractive`,
  `GenerateCommitMessage`, and `ExitCommitInput` in disjoint scopes
  (`keybindings.rs:816, 966, 1047`). Any change that flattens scope handling breaks it.

`detect_conflicts_default_config_clean` (`:2599-2607`) proves the shipped table never
self-conflicts. **It is the single most valuable guardrail for this phase** — if it goes
red, a `defs/*.rs` file was mis-partitioned.

## The safe splitting technique

Each `defs/*.rs` exports `pub const DEFS: &[BindingDef] = &[...]`; `mod.rs` reassembles:

```rust
pub static BINDING_DEFS: LazyLock<Vec<BindingDef>> = LazyLock::new(|| {
    [
        nav_and_projects::DEFS,
        agent_and_files::DEFS,
        global_and_overlays::DEFS,
        palette_only::DEFS,
    ]
    .concat()
});
```

Because `LazyLock<Vec<T>>` derefs to a slice, **every existing `BINDING_DEFS.iter()`
call site compiles unchanged** — including both in `config.rs`. This requires
`#[derive(Clone, Copy)]` on `BindingDef` / `HelpEntry` / `PaletteEntry`, which is free:
they hold only `&'static` data.

**The concatenation order above must reproduce the current declaration order exactly.**
Verify with a test that asserts the full ordered action sequence, not just the set.

## Module tree

```text
src/keys/
  mod.rs                    ~80   module docs, re-exports, HELP_SECTION_ORDER,
                                  BINDING_DEFS assembly (LazyLock + concat)
  types.rs                 ~466   Action (80 variants) + its three impl blocks, BindingScope,
                                  HintContext, HelpEntry, PaletteEntry, BindingDef (:8-466)
  defs/
    nav_and_projects.rs    ~304   Navigation + Projects sections (:498-801)
    agent_and_files.rs     ~254   Agent + Files + CommitInput (:802-1055)
    global_and_overlays.rs ~266   Global + Resize + Overlays (:1056-1321)
    palette_only.rs        ~238   the 13 palette-only actions (:1322-1559)
  format.rs                ~110   normalize_backtab, normalize_ctrl_punct, display_format,
                                  format_key, config_format, format_key_for_config,
                                  normalize_key_string (:1574-1638)
  runtime_lookup.rs        ~450   RuntimeBinding, RuntimeBindings, new, from_keys_config,
                                  lookup (:1645-1738)
  runtime_display.rs       ~270   label_for, labels_for, combined_label, hints_for,
                                  help_sections, filtered_palette (:1742-1857)
  conflicts.rs             ~195   KeyConflict, keys_conflict, resolve_keys, detect_conflicts (:1859-1957)
  interactive_bytes.rs     ~255   InteractiveByteBinding(s), interactive_byte_patterns,
                                  key_combination_to_bytes (:1958-2099)
```

`types.rs` at ~466 is the tightest. If Phase 10's D8 adds `Action` variants for the
three unbindable modals, split the `impl Action` blocks into `types/action_impl.rs`.

## Work items

1. **Confirm Phase 07's keybinding guardrails are green**: the action set/count pin
   (80 total, 13 palette-only), the `ctrl-g` three-scope test, and
   `detect_conflicts_default_config_clean`.
2. **Add `#[derive(Clone, Copy)]`** to `BindingDef`, `HelpEntry`, `PaletteEntry` as its
   own commit; confirm green.
3. **Add an ordered-sequence test** asserting the exact declaration order of all 80
   actions **before** splitting the table. This is the check that the `concat()` order
   is right, and it cannot be added afterward without begging the question.
4. **Extract `types.rs`, `format.rs`, `conflicts.rs`, `interactive_bytes.rs`** first —
   they are independent of the table split.
5. **Split `BINDING_DEFS` into the four `defs/*.rs`** using the `LazyLock` + `concat`
   technique. One commit. Run the ordered-sequence test and
   `detect_conflicts_default_config_clean` immediately.
6. **Split `RuntimeBindings`** into `runtime_lookup.rs` and `runtime_display.rs`. The
   lookup path is hot; keep `lookup` in one place and do not introduce indirection.
7. **Verify the external consumers still compile unchanged** —
   `config.rs::validate_keys` (`:3049`) and `render_keys_config`. If either needed
   editing, the `LazyLock` deref trick was not applied correctly.
8. **Add file headers** with generated trees per `CONVENTIONS.md` §1.
9. **Coordinate with Phase 10's D8** — if that phase adds `Action`s for the macro
   editor, macro bar, and help-overlay scroll, they land in `defs/global_and_overlays.rs`
   and the action-count pin updates in the same commit.

## Acceptance criteria

- [ ] `src/keybindings.rs` no longer exists; `src/keys/` matches the tree.
- [ ] No file exceeds 500 lines.
- [ ] `BINDING_DEFS` declaration order **identical** to pre-split, proven by the
      ordered-sequence test added in work item 3.
- [ ] `detect_conflicts_default_config_clean` and `lookup_declaration_order_wins` green.
- [ ] `ctrl-g` resolves to all three actions in their respective scopes.
- [ ] Action count is exactly 80, with 13 palette-only (or the new count, if Phase 10
      added variants — updated deliberately in the same commit).
- [ ] `config.rs`'s two `BINDING_DEFS` consumers compile **without modification**.
- [ ] Help overlay, palette, and hint bar render identically — verified against Phase 06
      snapshots with zero diff.
- [ ] File headers present; `module-trees --check` passes.
- [ ] `cargo test --all-features` green, same test count.

## Validation

```bash
cargo test --all-features keys
cargo test --all-features config          # the external consumers
cargo insta test                          # help/palette snapshots must not move
ci/check-file-length.sh
cargo modules dependencies --acyclic --bin dux

# prove order preservation explicitly
cargo test --all-features binding_defs_declaration_order
```

## Risks

| Risk | Mitigation |
|---|---|
| `concat()` order differs from declaration order | Work item 3's ordered-sequence test, added **before** the split |
| A `BindingDef` is dropped or duplicated across the four files | The action set/count pin from Phase 07, plus the ordered-sequence test |
| `LazyLock` changes the type enough to break external callers | Deref to slice keeps `.iter()` working; work item 7 is the explicit check |
| `#[derive(Copy)]` on a struct that later gains a non-`Copy` field | Acceptable now (all `&'static`); if a future field breaks it, revert to `&[&[BindingDef]]` and flatten at use |

## References

- `artifacts/research-config-keys-theme-cli.md` §2.2, §1.2, §5 (R7, R9)
