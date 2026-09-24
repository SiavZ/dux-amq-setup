# Phase 17 — `input.rs` and `render.rs` decomposition

**Track:** D (decomposition) · **Parallel-safe with:** 12–16 (coordinate with 16 on shared `input.rs` edits)
**Depends on:** 02, **05 (hard gate)**, **06 (hard gate)** · **Blocks:** 21, 24

## Goal

Split the two largest files — `render.rs` (7,178 production) and `input.rs` (6,163
production) — including the three giant functions inside them, with a safety net that
catches the specific failure mode this refactor risks.

## Why 06 is a hard gate

`render_prompt` (`render.rs:2279-5387`) is **3,109 lines, 25 modal arms, zero test
coverage**. Each arm ends by writing `self.ui.overlay_layout.active = …` — the **sole
contract** with `input.rs`'s mouse hit-testing (`input.rs:3823` onward). Drop or reorder
one during extraction and mouse support for that modal dies **silently**.

Compounding it, `handle_prompt_key` (`input.rs:1773-2872`) is an `if let` cascade of 39
checks ending in a bare `Ok(false)` (`:2871`) — **no exhaustiveness check**, so an
extraction that drops a block compiles cleanly. Phase 06 converts it to an exhaustive
`match` and adds the mouse-layout contract test. **Do not start this phase until both
are green.**

## The organizing seam

The dominant seam is already latent in the code: **`BindingScope` (12) × `PromptState`
(25)**. Every modal is currently smeared across **four** sites — a `PromptState` variant,
an `OverlayMouseLayout` variant, a `render_prompt` arm, and a `handle_prompt_key` block.
Making each modal **one module** collapses that, and is the single highest-value
structural change in the plan.

## Module tree

```text
src/app/input/
  dispatch.rs             ~260   handle_key (the 11-step early-return ladder, input.rs:277-529)
  panes/
    left.rs / center.rs / files.rs / resize.rs        ~645
  pty_raw.rs              ~420   process_raw_input_bytes
  pty_poll.rs             ~200
  scroll.rs               ~170
  mouse/
    hit_test.rs / dispatch.rs                          ~665
  prompt/
    dispatch.rs           ~450   skeleton + prompt_mouse_target
    name_new_agent.rs / kill_running.rs / session_settings.rs
    edit_macros.rs / browse_projects.rs / confirm_dialogs.rs   ~1,590
  command_palette.rs      ~100

src/app/render/
  mod.rs                  ~330   render(), layout, header, overlay dispatch, themed_block
  panes/
    left.rs / center.rs / agent_terminal.rs / files.rs
    macro_bar.rs / pty_colors.rs                       ~1,740
  footer.rs / pr_banner.rs / help.rs / resource_monitor.rs   ~970
  overlays/
    mod.rs                 ~50   render_prompt reduced to a dispatcher
    <one file per modal>.rs                          ~2,960  (largest: name_new_agent ~430)
    rows.rs                ~95   shared checkbox_line / radio_line / button_row
  session_settings.rs / edit_macros.rs                 ~745
```

Two tight spots: `render/overlays/name_new_agent.rs` (~430) and
`input/prompt/dispatch.rs` (~450, only if `prompt_mouse_target`'s 251-line match is
further broken up by overlay family). **Nothing genuinely resists a <500-line split.**

## Work items

1. **Confirm Phase 06 is green**: all 25 modal snapshots at two widths, the mouse-layout
   contract test compiling exhaustively over `PromptState`, and `handle_prompt_key`
   converted to a `match`.
2. **Extract the pure helpers from `render.rs` first** — the 24 free functions that
   already have all 43 of the file's tests. Zero risk, immediate size win, validates tooling.
3. **Apply the reusable extractions before splitting**, so duplication is removed rather
   than multiplied across new files:
   - **R5** popup list-panel border — **8 sites** (`render.rs:2364, 2537, 2721, 2900, 3075,
     3186, 3348, 6354`) → `render_overlay_list_below_input(..) -> Rect`.
   - **R6** hint/footer bar spans — a helper exists but is scoped to one family
     (`edit_macro_hints`, `render.rs:6144`, 4 uses) while **11 sites are inline**
     (`render.rs:2333, 2486, 2563, 2621, 2804, 2982, 3109, 3291, 5359, 6274`). Lift it.
   - **R3** checkbox — **three parallel implementations**: `components/checkbox.rs:39`
     (5 uses), the from-scratch free fn `checkbox_line` (`render.rs:6882`, 8 uses), and
     inline `"[x]"/"[ ]"` (`render.rs:4542`). Consolidate on `components::Checkbox`.
   - **R4** button — **two implementations with different visuals**: `components::Button`
     (rounded border, 24 uses) and `button_row` (`render.rs:6904`, plain `[ Save ]`,
     hardcoded labels, 1 use at `:5717`). Consolidate on `components::Button`.
     **Note: this is a visible change** for that one call site — expect a snapshot diff
     and confirm it is the intended unification.
   - **R9** string truncation — **four implementations, three weaker**. Route all through
     the width-aware, tested `truncate_status_text` (`render.rs:7156`); the char-count-only
     versions at `render.rs:2311, 3229, 6338` are the ones to retire. Phase 06 item 7
     snapshotted these at truncating widths precisely to prove this safe.
   - **R1** list nav clamp/wrap — **11 sites** (`input.rs:531-552, 606-611, 1898-1918,
     2085-2103, 2289-2311, 1948-1953, 2372-2377, 2404-2410, 2457-2463, 2503-2510,
     2532-2537`) → `fn move_index(sel: &mut usize, len: usize, down: bool) -> bool`.
   - **R2** bool confirm-dialog boilerplate, ~19 lines each — **6 sites**
     (`input.rs:2554, 2640, 2661, 2681, 2762, 2782`) → one generic handler.

   R1+R2+R5+R6 alone remove ~500 lines and, more importantly, **31 independent places an
   off-by-one or a styling drift can hide.**
4. **Split `render_prompt` into `overlays/<modal>.rs`**, one modal per commit, running
   the contract test and snapshots after **each**. Reduce `render_prompt` itself to a
   dispatcher.
5. **Split `handle_prompt_key` into `input/prompt/<modal>.rs`**, pairing each with its
   render counterpart from step 4 so the two halves of a modal land together.
6. **Split the panes, mouse, and raw-input paths.**
7. **Address `PromptState` / `OverlayMouseLayout` being hand-synced parallel enums.** With
   one module per modal, make the pairing explicit — a trait or a macro that emits both, or
   at minimum a compile-time assertion that every `PromptState` variant with a mouse layout
   has a matching `OverlayMouseLayout` variant. **Six modals legitimately have none**
   (`WatchRules`, `EditMacros`, `ConfirmSharedWriter`, `OrphanWorktrees`,
   `ConfirmRemoveOrphanWorktree`, `DebugInput`) — encode that list, do not silently allow gaps.
8. **Make the palette compile-time-linked.** `BINDING_DEFS` already carries
   `palette: Option<PaletteEntry>` (`keybindings.rs:184`), but selecting a row passes
   `binding.palette_name` — a `&'static str` — to `execute_command` (`mod.rs:2278-2452`),
   a hand-written match over ~41 string literals with an `"Unknown command"` fallback
   (`:2431`). **Nothing links the two tables at compile time**, which is how
   `delete-terminal` became dead (Phase 10 D3). Dispatch on `Action`, not on a string, so
   a missing arm is a compile error.
9. **Add file headers** with generated trees per `CONVENTIONS.md` §1.
10. **Coordinate `input.rs` edits with Phase 16.** R7/R8/R11 live in `input.rs` and are
    listed in Phase 16. **Decide ownership before either starts.** Recommended: Phase 16
    takes R7/R8/R11 (they are session/pane logic), Phase 17 takes R1/R2 (they are
    prompt/list logic).

## Acceptance criteria

- [ ] `src/app/input.rs` and `src/app/render.rs` no longer exist; the trees above match.
- [ ] No file exceeds 500 lines.
- [ ] `render_prompt` and `handle_prompt_key` are dispatchers under 60 lines each.
- [ ] The mouse-layout contract test passes for all 25 `PromptState` variants.
- [ ] **All Phase 06 snapshots show zero diff**, except the single intended `button_row`
      unification (R4) — which must be explicitly reviewed and noted in the PR.
- [ ] R1, R2, R3, R4, R5, R6, R9 applied; the duplicate sites are gone (grep verification).
- [ ] Exactly one checkbox implementation and one button implementation exist.
- [ ] All truncation routes through `truncate_status_text`.
- [ ] `execute_command` dispatches on `Action`, not strings; a missing arm is a compile error.
- [ ] The six keyboard-only modals are encoded as an explicit list, not an implicit gap.
- [ ] `cargo test --all-features` green, same test count; 236 `input.rs` behavioural tests pass.
- [ ] File headers present; `module-trees --check` passes.

## Validation

```bash
cargo test --all-features
cargo insta test                    # zero diff except the reviewed R4 change
ci/check-file-length.sh
cargo modules orphans --deny --bin dux
cargo modules dependencies --acyclic --bin dux

# the contract test must still catch its failure mode after the split:
#   delete one `ui.overlay_layout.active = ...` write -> test MUST fail -> revert
```

## Risks

| Risk | Mitigation |
|---|---|
| A dropped `overlay_layout.active` write kills mouse support silently | Phase 06's contract test, run after **every** modal extraction — this is the whole reason 06 gates 17 |
| The `if let` cascade's fallthrough order is load-bearing | Phase 06 converts it to a `match` **preserving arm order exactly**, under snapshots, before this phase begins |
| R3/R4 consolidation changes visuals | R4 changes one call site deliberately; R3's three impls must be compared pixel-by-pixel via snapshots before choosing the survivor |
| 25 modals × 2 commits each is a long series | That is the correct pace. One modal per commit keeps every failure attributable to ~120 lines |
| Phase 16 and 17 both edit `input.rs` | Work item 10 — assign R-numbers to exactly one phase before starting |

## References

- `artifacts/research-app-monoliths.md` §Decomposition seams, §The four giants,
  §Reusable extractions R1–R9, §C3, §C4, §C5, §Risk register (breakage vectors 4–5)
