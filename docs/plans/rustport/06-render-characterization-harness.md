# Phase 06 — Render characterization harness

**Track:** B (safety net) · **Parallel-safe with:** 05, 07
**Depends on:** 05 (uses `App::for_test()`) · **Blocks:** 17 — **hard gate**

## Goal

Build the missing safety net before anyone touches the renderer. Today the largest
production file in the codebase has effectively zero coverage of what it produces.

## Evidence

- **`render.rs` is 7,178 production lines with 43 tests, and every one covers a pure
  helper** — `centered_rect_exact`, `capitalize`, `format_bytes`,
  `truncate_status_text`, `to_grayscale`, `xterm256_*`, `wrapped_line_count`,
  `pty_cell_colors`. **Zero tests call any `render_*` method.**
- **`render.rs` uses `TestBackend` zero times**, while `input.rs` uses it 7 times.
- **`render_prompt` (`render.rs:2279-5387`) is 3,109 lines, 25 modal arms, and has no
  coverage whatsoever.** It is 41% of the file.
- **The silent-failure mechanism:** each of those 25 arms ends by writing
  `self.ui.overlay_layout.active = …`. That write is the **sole contract** with
  `input.rs`'s mouse hit-testing (`input.rs:3823` onward). Dropping or reordering one
  during extraction kills mouse support for that modal — and **nothing would catch it**.
- Compounding it, `handle_prompt_key` (`input.rs:1773`) is an `if let` cascade of 39
  checks ending in a bare `Ok(false)` (`:2871`) — **no exhaustiveness check**, so an
  extraction that drops a block compiles cleanly.
- Six modals have no `OverlayMouseLayout` variant at all and are keyboard-only:
  `WatchRules`, `EditMacros`, `ConfirmSharedWriter`, `OrphanWorktrees`,
  `ConfirmRemoveOrphanWorktree`, `DebugInput`.
- Dev-dependencies today are only `filetime` and `serial_test` — **no `insta`,
  `proptest`, or `rstest`**.

## In scope

Golden-buffer coverage for every modal and pane, a mouse-layout contract test, and
the `PromptState` exhaustiveness fix that makes future additions fail loudly.

## Out of scope

- Splitting `render.rs` or `input.rs` → Phase 17.
- Fixing the modals that lack mouse support → Phase 10 decides; this phase only
  *records* which six lack it.

## Decision: add `insta`

Hand-rolled `TestBackend` assertions work at small scale, but 25 modals × multiple
states is exactly where snapshot review tooling pays for itself: `cargo insta review`
turns "did this 60×20 buffer change correctly?" into a diff a human can approve.
`insta` is widely used, actively maintained, and dev-only — it does not enter the
shipped binary or the SBOM's runtime graph.

Use `insta` for **buffer snapshots**; use plain assertions for the **structural
contract** (work item 3), because that must fail on a *missing* write, which a
snapshot of the buffer cannot see.

## Work items

1. **Add `insta` as a dev-dependency** and configure `cargo insta` in
   `docs/contributing/`. Verify it clears `deny.toml`'s license allowlist.
2. **Write a `render_to_string(app, w, h)` helper** over `ratatui::backend::TestBackend`
   that renders one frame and returns the buffer as text with styles rendered as
   annotations. This is the primitive everything else uses.
3. **Write the mouse-layout contract test — the highest-value item in this phase.**
   For every `PromptState` variant: construct it, render one frame, and assert
   `ui.overlay_layout.active` is **not** `OverlayMouseLayout::None` unless the variant
   is on the documented keyboard-only list (the six above). Drive it from an
   exhaustive `match` over `PromptState` so **adding a variant fails to compile**
   rather than silently skipping. This is the test that catches the exact failure mode
   Phase 17 risks.
4. **Snapshot every `render_prompt` arm** — all 25 variants, at a minimum two terminal
   sizes (a narrow 60×20 and a wide 200×50) to catch layout math that only breaks at
   one width. Include the variants that embed sub-state (`ConfirmKillRunning { previous }`,
   `EditMacros { pending_delete }`) in both their base and nested forms.
5. **Snapshot the panes and chrome**: left pane (collapsed and expanded projects),
   center in each `CenterMode`, files pane (staged/unstaged/empty), the diff overlay
   with and without line numbers, header, footer, PR banner in both positions, help
   overlay, macro bar, and the resource monitor.
6. **Fix the `handle_prompt_key` exhaustiveness gap.** Convert the 39-check `if let`
   cascade at `input.rs:1773-2872` into a `match` over `PromptState` with no `_` arm,
   so it mirrors `render_prompt`'s compiler-enforced exhaustiveness. This is a
   prerequisite for safe extraction, not a nicety — do it here, under the new
   snapshots, rather than during the split.
   **Note:** this is a behavioural-risk change. Land it as its own commit with the
   snapshots already green, and verify no key handling changes by running the 236
   existing `input.rs` behavioural tests.
7. **Snapshot the width-dependent truncation paths.** `truncate_status_text`
   (`render.rs:7156`) is width-aware and tested; three other call sites
   (`render.rs:2311, 3229, 6338`) are char-count-only. Snapshot all four at a width
   that forces truncation, so Phase 17's consolidation onto the tested helper (R9) is
   provably behaviour-preserving.
8. **Document the snapshot workflow** in `docs/contributing/`: how to review, when
   accepting a changed snapshot is legitimate, and the rule that a snapshot change in
   a *pure refactor* PR is a defect until explained.

## Acceptance criteria

- [ ] `insta` added as a dev-dependency and passing `cargo deny`.
- [ ] `render_to_string` helper exists and is used by all snapshot tests.
- [ ] The mouse-layout contract test covers **all 25** `PromptState` variants via an
      exhaustive `match`, and fails to compile if a variant is added.
- [ ] The six keyboard-only modals are listed explicitly in that test, with a comment
      linking to the decision (not silently skipped).
- [ ] Snapshots exist for all 25 `render_prompt` arms at two terminal widths.
- [ ] Snapshots exist for every pane, overlay, and chrome element in work item 5.
- [ ] `handle_prompt_key` is an exhaustive `match` with no `_` arm; all 236 `input.rs`
      behavioural tests still pass.
- [ ] Deliberately deleting one `overlay_layout.active` write makes the contract test
      fail — **verify this by actually doing it and reverting.**
- [ ] Deliberately reordering two `render_prompt` arms produces a snapshot diff.
- [ ] `cargo test --all-features` green.

## Validation

```bash
cargo test --all-features
cargo insta test --review          # first run accepts the baseline
cargo insta test                   # subsequent runs must be clean

# prove the net actually catches the failure it exists for:
#   1. delete one `self.ui.overlay_layout.active = ...` line in render_prompt
#   2. cargo test  -> the mouse-layout contract test MUST fail
#   3. git checkout the file
```

## Risks

| Risk | Mitigation |
|---|---|
| Snapshots become noise nobody reviews | Keep them narrow: one modal per snapshot, small viewports. The contract test (item 3) is assertion-based precisely so the critical property does not depend on snapshot review |
| Converting the `if let` cascade to a `match` changes key handling | Its own commit, snapshots already green, 236 behavioural tests as the check. The cascade's fallthrough order is load-bearing — preserve arm order exactly |
| Snapshot churn on unrelated theme changes | Snapshot text plus explicit style annotations rather than raw ANSI; theme edits then touch only the annotation line |
| 25 modals × 2 widths is a lot of fixture setup | `App::for_test()` from Phase 05 exists specifically to make this cheap — that is why 05 gates this phase |

## References

- `artifacts/research-app-monoliths.md` §Risk register ("would move blind", breakage vector 4), §C3, §C4
- `artifacts/research-external-patterns.md` §Testing the decomposition: yes, add `insta`
