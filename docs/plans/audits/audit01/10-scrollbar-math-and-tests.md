# Phase 10: Scrollbar math + snapshot tests

> Maps to audit findings: P1-4, P2-3 (partial)

## Goal
Fix the `ScrollbarState` overcount in `src/app/render.rs:1352-1368`. Today
`content_length = total + visible` and `viewport_content_length(visible)`
overcounts by `visible` so the thumb never reaches the bottom. Replace
with the documented-clean form and add a snapshot test (also closes part
of P2-3 — the patches had no Rust tests).

## Pre-conditions
- Phase 06 extracted `patches/0003-scrollbar.diff`.
- Phase 00 baseline test green.

## Files to touch
- `src/app/render.rs` — modify the scrollbar block.
- `tests/scrollbar_render.rs` — fill out (placeholder from Phase 00).
- `patches/0003-scrollbar.diff` — regenerate.

## Steps
1. **Verification needed before implementation**: re-read pinned ratatui's
   ScrollbarState source (`grep -nR 'fn position\|fn content_length\|fn
   viewport_content_length' ~/.cargo/registry/.../ratatui-*/src/widgets/scrollbar`).
   ratatui issue #1493 documents `viewport_content_length` as "track-size
   fallback only". Confirm before finalising.
2. Apply the simpler form. dux offset semantics (0 = latest at bottom)
   require `position = total - offset - 1`, saturating, when `total > 0`.
   ```diff
   - let mut state = ScrollbarState::new(total + visible)
   -     .viewport_content_length(visible)
   -     .position(position);
   + // dux offset 0 ⇒ latest line at bottom ⇒ ratatui position = total - 1
   + let position = total.saturating_sub(offset).saturating_sub(1);
   + let mut state = ScrollbarState::new(total).position(position);
   ```
3. Snapshot test using ratatui's `TestBackend`:
   ```rust
   // tests/scrollbar_render.rs
   use ratatui::{Terminal, backend::TestBackend, widgets::*};
   #[test]
   fn thumb_at_bottom_when_offset_zero() {
       let mut term = Terminal::new(TestBackend::new(20, 10)).unwrap();
       let total = 100usize; let offset = 0usize;
       let position = total.saturating_sub(offset).saturating_sub(1);
       let mut state = ScrollbarState::new(total).position(position);
       term.draw(|f| {
           Scrollbar::new(ScrollbarOrientation::VerticalRight)
               .render(f.area(), f.buffer_mut(), &mut state);
       }).unwrap();
       let buf = term.backend().buffer().clone();
       let top    = buf.cell((19, 0).into()).unwrap().symbol().to_string();
       let bottom = buf.cell((19, 9).into()).unwrap().symbol().to_string();
       assert_ne!(top, bottom, "thumb should be at bottom when offset=0");
   }
   ```
   Add tests for `offset = total - 1` (thumb at top) and small-content
   (`total < visible` — should be a no-op render given the existing
   guard; assert the column buffer is empty).
4. Regenerate `patches/0003-scrollbar.diff`.

## Validation
- `cargo test --test scrollbar_render` passes.
- Manual: pane with ≥1000 lines; press End / Home; thumb flush
  bottom / top.
- `cargo clippy --all-targets -- -D warnings` clean.

## Acceptance criteria
- [x] `ScrollbarState::new(total).position(position)` form replaces `+ visible`.
- [x] `viewport_content_length` no longer called.
- [x] `position = total - offset - 1` (saturating).
- [x] Snapshot tests cover offset=0 (thumb at bottom), offset=total-1 (thumb at top), middle, and content_length=0 short-circuit. Verified against the pinned `ratatui-widgets-0.3.0` source under `~/.cargo/registry/.../ratatui-widgets-0.3.0/src/scrollbar.rs::part_lengths` rather than docs.rs.
- [ ] `patches/0003-…` regenerated; applies cleanly to upstream/main. *(Track C scope is Rust only — `patches/` regeneration belongs to the wrapper-chain track and is deferred.)*

## References
- Audit P1-4, P2-3.
- ratatui #1493 (docs): https://github.com/ratatui/ratatui/issues/1493
- ratatui #966 (viewport assumption): https://github.com/ratatui/ratatui/issues/966
- ScrollbarState docs: https://docs.rs/ratatui/latest/ratatui/widgets/struct.ScrollbarState.html
