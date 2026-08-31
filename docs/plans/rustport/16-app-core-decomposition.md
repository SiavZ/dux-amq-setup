# Phase 16 — `src/app/` core decomposition: types, bootstrap, sessions, workers

**Track:** D (decomposition) · **Parallel-safe with:** 12, 13, 14, 15, 17
**Depends on:** 02, **05 (hard gate)** · **Blocks:** 21, 24

## Goal

Split `app/mod.rs` (4,158 production), `app/sessions.rs` (2,820), and `app/workers.rs`
(3,306) into modules under the cap, and move the ~1,278 lines of type definitions out of
`mod.rs` — which is what makes every other `src/app/` split possible.

## Why 05 is a hard gate

The production `App { .. }` literal is **137 lines** (`mod.rs:1598-1647`), which makes
`bootstrap_with_lock` unsplittable, and it is duplicated in **two** hand-rolled test
fixtures (`input.rs:6374`, `sessions.rs:2955`). Any field move breaks both. Phase 05
unifies them into one `App::for_test()` builder first.

**Do the field migration before the file split.** Folding the ~20 loose UI fields into
`UiState`, the three raw-input buffers into a new `InputState`, and the two stray
in-flight flags into `GitState`/`RuntimeState` shrinks the literal to ~15 lines and cuts
the churn of every subsequent move.

## Current shape

- `mod.rs` regions: imports 1-56 · `App` struct 58-125 · **66 type definitions
  127-1404 (~1,278 lines)** · **`mod` declarations at 1406-1413, mid-file** ·
  `impl App` 1446-3921 · free fns 3923-4157.
- Largest types: `WorkerEvent` (`:1173-1376`, 40 variants), `PromptState`
  (`:480-608`, 25 variants), `OverlayMouseLayout` (`:955-1066`, 21 variants).
- `App` has **48 fields**: 3 sub-struct handles + **45 still loose**. Sub-structs hold 72
  more (`RuntimeState` 33, `UiState` 22, `GitState` 17). All `pub(crate)` except
  `last_snapshot_id` (`mod.rs:117`).
- **373 methods across 9 `impl App` blocks.** Rust permits many `impl App` blocks across
  sibling modules — the codebase already does this in 9 files — so **no trait objects,
  newtypes, or dyn dispatch are needed.**
- `drain_events` (`workers.rs:8-1062`) is **1,055 lines**: one `try_recv` loop wrapping a
  40-arm match, plus a 160-line unconditional epilogue (`:904-1061`).
- `run_create_agent_job` (`workers.rs:2359-2861`) is **503 lines**: a 260-line 3-arm match
  returning an **11-tuple**, then a linear pipeline.
- `sessions.rs` is the outlier — long but not tangled; max method 140 lines
  (`continue_reconnect`, `:1946-2085`). **Cheapest file in the phase; start here.**

## Module tree

```text
src/app/
  mod.rs                    ~120   App struct + mod decls (moved to the top) + re-exports
  types/
    panes.rs / prompts.rs / dialogs.rs / events.rs / mouse.rs
                          ~1,160   the 66 types from mod.rs:127-1404 (each file 130-300)
  bootstrap.rs              ~330   bootstrap_with_lock, restore_sessions, load_projects
  run_loop.rs               ~165   App::run
  status.rs                 ~150   close_top_overlay, set_info/busy/warning/error, spinner
  auto_resume.rs            ~290
  resource_monitor.rs       ~330
  commands.rs               ~245   execute_command
  left_pane.rs / files_pane.rs / pty_lifecycle.rs / watch_runtime.rs / terminals.rs
                          ~1,525   the rest of mod.rs's impl App (each 190-410)

  sessions/                ~2,410  projects.rs · create.rs · terminals.rs · delete.rs
                                   provider_theme.rs · reconnect.rs · diff_open.rs
                                   kill_running.rs · settings.rs   (each 150-340)

  workers/
    mod.rs                  ~120   drain_events reduced to a 40-line dispatcher
    events/                ~1,000  agent_lifecycle.rs · git_repo.rs · project_admin.rs
                                   session_teardown.rs · github.rs · resources.rs
    reaping.rs ~200 · watchdogs.rs ~250 · github.rs ~430 · pr_query.rs ~290
    resume.rs ~380 · create_agent.rs ~460 · dispatch_git.rs + dispatch_session.rs ~520
    config_save.rs ~230 · misc_dispatch.rs ~230
```

`app/orchestrator.rs` (334 production) is **already compliant — leave it alone.**

## Work items

1. **Field migration first, as its own commit series.** Fold the ~20 loose UI fields into
   `UiState`, add `InputState` for the three raw-input buffers, and move
   `create_agent_in_flight` / `resource_stats_in_flight` (`mod.rs:87-88`) into
   `GitState`/`RuntimeState` beside the 7 identical in-flight markers already there.
   Also correct two misplacements: `last_pty_activity` (PTY state) and
   `collapsed_projects` (UI state) currently live in `GitState`.
2. **Move `mod` declarations to the top of `mod.rs`** (currently at `:1406-1413`, after
   1,278 lines of types). Cosmetic, but do it before the type extraction so the diff is
   readable.
3. **Extract `types/`** — the 66 definitions from `mod.rs:127-1404`. Group by concern, not
   by size. `WorkerEvent`, `PromptState`, and `OverlayMouseLayout` are the anchors of
   `events.rs`, `prompts.rs`, and `mouse.rs` respectively.
4. **Split `sessions/` next** — lowest risk, highest confidence, validates the tooling.
5. **Split `workers/`.** Reduce `drain_events` to a ~40-line dispatcher over the six
   `events/*.rs` modules. **The 160-line unconditional epilogue (`:904-1061`) must keep
   running exactly once per drain** — it is not part of the match, and folding it into a
   handler would change semantics.
6. **Refactor `run_create_agent_job`'s 11-tuple** (`workers.rs:2359-2861`). Returning an
   11-tuple from a 260-line match is the reason the function cannot be split. Introduce a
   named struct for the result; the pipeline then decomposes naturally.
7. **Widen `last_snapshot_id`** (`mod.rs:117`) to `pub(crate)` — it is the only private
   field, and moving `refresh_snapshot_buf` out of `mod.rs` requires it.
8. **Re-check every moved method's visibility.** 373 methods are currently siblings in one
   module tree; each needs its `pub(crate)`/private status reconsidered rather than blanket
   `pub(crate)`. Prefer the narrowest that compiles.
9. **Apply the reusable extractions** identified in the research — each removes duplication
   rather than merely relocating it:
   - **R8** "enter interactive or reconnect" — 5 sites (`input.rs:578-596, 653-661,
     5486-5495, 5502-5511, 5528-5536`) → one `enter_agent_or_reconnect()`.
   - **R11** key-vs-mouse handler pairs duplicating one formula — `input.rs:1826`≡`5062`,
     `:1878`≡`5111`.
   - **R7** the 3-way focus-cycle state machine — 3 sites (`input.rs:2596-2606, 2716-2739,
     1957-1968`); the index-into-`Vec` pattern is **already solved** at `:2984-3006`.
10. **Add file headers** with generated trees per `CONVENTIONS.md` §1.

## Acceptance criteria

- [ ] `App` has **no loose UI fields**; the `App { .. }` literal is under 20 lines.
- [ ] `mod.rs` under 500 lines; `mod` declarations at the top.
- [ ] `types/`, `sessions/`, `workers/` trees match the plan; no file over 500 lines.
- [ ] `drain_events` is a dispatcher under 60 lines; the epilogue runs exactly once per
      drain (assert with a counter test).
- [ ] `run_create_agent_job` returns a named struct, not an 11-tuple, and is under 500 lines.
- [ ] R7, R8, R11 applied; duplicate sites gone (`grep` verification in the PR).
- [ ] No method's visibility widened beyond what compiles.
- [ ] `app/orchestrator.rs` untouched.
- [ ] Phase 06 snapshots show zero diff.
- [ ] `cargo test --all-features` green, same test count.
- [ ] File headers present; `module-trees --check` passes.

## Validation

```bash
cargo test --all-features
cargo insta test                     # zero snapshot movement
ci/check-file-length.sh
cargo modules orphans --deny --bin dux
cargo modules dependencies --acyclic --bin dux

# epilogue-runs-once guard
cargo test --all-features drain_events_epilogue
```

## Risks

| Risk | Mitigation |
|---|---|
| The 137-line `App` literal breaks both test fixtures on every field move | Phase 05 unifies them first — that is why it is a hard gate |
| `drain_events`' epilogue silently runs zero or twice | Explicit counter test in the acceptance criteria, not inspection |
| Visibility churn across 373 methods turns into blanket `pub(crate)` | Work item 8; review each. `cargo modules` surfaces over-exposure |
| Phase 17 touches `input.rs` concurrently for R7/R11 | R7/R11 sites live in `input.rs`; coordinate — either 16 lands them first and 17 rebases, or 17 owns them. **Decide before starting, do not both edit** |

## References

- `artifacts/research-app-monoliths.md` §Decomposition seams, §The god-object, §Worker model,
  R7, R8, R11, §Risk register
