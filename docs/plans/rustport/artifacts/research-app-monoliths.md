# Research brief — `src/app/` structure and decomposition seams

Source: parent research agent A (5 sub-agents), 2026-08-31. Every load-bearing
claim was re-verified against source by the parent; unverified claims were
dropped and appear as open questions.

## Verified structure

### Real line counts — the monolith is 32% test code

| File | Total | **Prod** | Test | #tests | test/prod |
|---|---|---|---|---|---|
| `render.rs` | 7,565 | **7,178** | 386 | 43 | **0.05** |
| `input.rs` | 13,266 | **6,163** | 7,102 | 236 | 1.15 |
| `mod.rs` | 4,447 | **4,158** | 289 | 13 | 0.07 |
| `workers.rs` | 4,394 | **3,306** | 1,088 | 16 | 0.33 |
| `sessions.rs` | 4,713 | **2,820** | 1,893 | 53 | 0.67 |
| `inject_runtime.rs` | 1,756 | 1,307 | 449 | 37 | 0.34 |
| `text_input.rs` | 1,804 | 768 | 1,036 | 93 | 1.35 |
| `orchestrator.rs` | 436 | 334 | 102 | 5 | 0.31 |
| `components/{button,checkbox}.rs` | 704 | 472 | 232 | 17 | 0.49 |
| `state/*.rs` + `components/mod.rs` | 314 | 314 | 0 | 0 | — |
| **Total** | **39,399** | **26,820** | 12,579 | 513 | 0.47 |

**`render.rs` — not `input.rs` — is the largest production file**, and it is the
least tested by a factor of 10.

### The god-object, measured

- `App` struct: `mod.rs:58-125` — **48 fields = 3 sub-struct handles + 45 still loose**.
  All `pub(crate)` except `last_snapshot_id` (`mod.rs:117`).
- Sub-structs hold 72 more fields: `RuntimeState` 33 (`state/runtime.rs:21-150`),
  `UiState` 22 (`state/ui.rs:13-45`), `GitState` 17 (`state/git.rs:17-76`).
- **373 methods across 9 `impl App` blocks** (mod 106, input 110, sessions 77,
  render 33, workers 29, inject_runtime 15, orchestrator 3).
- **Zero `impl UiState` / `impl GitState` / `impl RuntimeState` blocks exist
  anywhere in the crate.** The only `fn` in `state/` is `RuntimeState::drop`
  (`runtime.rs:152-156`).

### `mod.rs` (4,158 prod) — region map

imports 1-56 · `App` struct 58-125 · **66 type definitions 127-1404 (~1,278 lines)** ·
**`mod` declarations 1406-1413 (mid-file)** · `impl App` 1446-3921 (2,476) ·
free fns 3923-4157 · tests 4159-4447.

Biggest types: `WorkerEvent` 1173-1376 (204 ln, 40 variants),
`PromptState` 480-608 (25 variants), `OverlayMouseLayout` 955-1066 (21 variants).

### Run loop — `App::run`, `mod.rs:1671-1835` (165 ln)

Pre-spawns 7 workers (`mod.rs:1675-1688`). Per iteration: `drain_events` →
`poll_pty_activity` → `tick_watch_engines` → `tick_amq_inject` →
`tick_orchestrator_watchdog` → `tick_count += 1` (`mod.rs:1694-1699`) →
SIGWINCH (`1703`) → `force_redraw` (`1709`) → `terminal.draw` (`1719`).

- Two mutually exclusive event sources: crossterm `event::poll(100ms)`/`read`
  (`mod.rs:1754/1768`) when non-interactive; raw `rustix` stdin poll via
  `poll_and_forward_raw_input` (`input.rs:1199`) when `input_target` is
  `Agent`/`Terminal`. No input thread — both block the UI thread. Cadence ≈10 Hz idle.
- **Redraw is unconditional every iteration.** `force_redraw` is *not* a dirty
  flag — it only adds a `terminal.clear()` (`mod.rs:1709-1717`).

### Key dispatch — genuinely well-structured

`handle_key` (`input.rs:277-529`) is an 11-step early-return ladder:
prompt(281) → macro bar(285) → interactive `debug_assert`(288) →
Global `CloseOverlay`(297) → help(302) → files-search(331) → commit input(338) →
`defer_global`(344) → Global match(356) → resize(504) → `match self.ui.focus`(509).

Backing it: a real `Action` enum (80 variants, `keybindings.rs:8`), `BindingScope`
(12 variants, `:102`), and a declarative `BINDING_DEFS` table (`:498`) carrying
default keys, scopes, help, hints **and** palette entry per action.
`bindings.lookup(&key, scope) -> Option<Action>` then `match action`:
41 lookup sites, 22 match blocks in prod.

### Worker model — one channel, one enum, one drain

`std::sync::mpsc` only. `worker_tx`/`worker_rx` at `state/runtime.rs:22-23`.
**One** `WorkerEvent` enum (40 variants) carries git, PTY, PR, config, disk, AMQ
and cleanup results. `drain_events` (`workers.rs:8-1062`, **1,055 lines**) = one
`try_recv` loop wrapping one 40-arm match, plus a 160-line unconditional epilogue
(`904-1061`). Uncapped drain.

**35 `thread::spawn` sites, thread-per-task, no pool.** The only concurrency bound
in the codebase is `MAX_PR_CHECKS_IN_FLIGHT = 4` (`workers.rs:1610`). One dedicated
long-lived thread exists (config-save worker, `workers.rs:3146`).

### The four giants (only functions >500 lines)

| Function | Location | Lines | Shape |
|---|---|---|---|
| `render_prompt` | `render.rs:2279-5387` | **3,109** | exhaustive `match`, 25 arms — 41% of render.rs |
| `handle_prompt_key` | `input.rs:1773-2872` | **1,100** | **`if let` cascade, 39 checks — not a match** |
| `drain_events` | `workers.rs:8-1062` | **1,055** | 40-arm match + epilogue |
| `run_create_agent_job` | `workers.rs:2359-2861` | **503** | 260-line 3-arm match returning an 11-tuple, then a linear pipeline |

`sessions.rs` is the outlier: max method 140 lines (`continue_reconnect`,
`1946-2085`). It is long, not tangled — the cheapest file to split.

## Decomposition seams

The dominant seam is already latent: **`BindingScope` (12) × `PromptState` (25)**.
Every modal is currently smeared across 4 sites — a `PromptState` variant, an
`OverlayMouseLayout` variant, a `render_prompt` arm, and a `handle_prompt_key`
block. Making each modal one module collapses that.

```
src/app/
  mod.rs                    ~120   App struct + mod decls + re-exports
  types/{panes,prompts,dialogs,events,mouse}.rs
                          ~1,160   the 66 types now at mod.rs:127-1404 (each 130-300)
  bootstrap.rs              ~330   bootstrap_with_lock, restore_sessions, load_projects
  run_loop.rs               ~165   App::run
  status.rs                 ~150   close_top_overlay, set_info/busy/warning/error, spinner
  auto_resume.rs            ~290
  resource_monitor.rs       ~330
  commands.rs               ~245   execute_command (see C5 — should become Action-typed)
  left_pane.rs / files_pane.rs / pty_lifecycle.rs / watch_runtime.rs / terminals.rs
                          ~1,525   rest of mod.rs's impl App (each 190-410)

  input/
    dispatch.rs             ~260   handle_key
    panes/{left,center,files,resize}.rs        ~645
    pty_raw.rs              ~420   process_raw_input_bytes
    pty_poll.rs             ~200
    scroll.rs               ~170
    mouse/{hit_test,dispatch}.rs               ~665
    prompt/dispatch.rs      ~450   skeleton + prompt_mouse_target (may need further split)
    prompt/{name_new_agent,kill_running,session_settings,edit_macros,
            browse_projects,confirm_dialogs}.rs  ~1,590
    command_palette.rs      ~100

  render/
    mod.rs                  ~330   render(), layout, header, overlay dispatch, themed_block
    panes/{left,center,agent_terminal,files,macro_bar,pty_colors}.rs   ~1,740
    footer.rs / pr_banner.rs / help.rs / resource_monitor.rs           ~970
    overlays/mod.rs          ~50   render_prompt reduced to a dispatcher
    overlays/<one per modal>.rs                ~2,960 (largest: name_new_agent ~430)
    overlays/rows.rs         ~95   shared checkbox_line/radio_line/button_row (R3)
    session_settings.rs / edit_macros.rs       ~745

  workers/
    mod.rs                  ~120   drain_events reduced to a 40-line dispatcher
    events/{agent_lifecycle,git_repo,project_admin,session_teardown,
            github,resources}.rs               ~1,000
    reaping.rs ~200 · watchdogs.rs ~250 · github.rs ~430 · pr_query.rs ~290
    resume.rs ~380 · create_agent.rs ~460 · dispatch_{git,session}.rs ~520
    config_save.rs ~230 · misc_dispatch.rs ~230

  sessions/  {projects,create,terminals,delete,provider_theme,reconnect,
              diff_open,kill_running,settings}.rs   ~2,410 (each 150-340)

  inject_runtime/ {watcher,queue_scan,tick,deliver,diagnostics}.rs ~870
  orchestrator.rs           436   already compliant — leave alone
```

**Nothing genuinely resists a <500-line split.** Tight spots:
`render/overlays/name_new_agent.rs` (~430) and `input/prompt/dispatch.rs` (~450,
only if `prompt_mouse_target`'s 251-line match is further broken up by overlay
family). Rust permits many `impl App` blocks across sibling modules — the codebase
already does this in 9 places — so no trait objects or newtypes are needed.

**Sequencing matters.** Do the *field* migration before the *file* split. The
137-line `App { .. }` literal at `mod.rs:1598-1647` is what makes
`bootstrap_with_lock` unsplittable, and it is duplicated in two test fixtures.
Folding the ~20 loose UI fields into `UiState`, the 3 raw-input buffers into a new
`InputState`, and the two stray in-flight flags into `GitState`/`RuntimeState`
shrinks that literal to ~15 lines and cuts the churn of every subsequent move.

## Reusable extractions

| # | Duplication | Sites | Evidence | Replacement |
|---|---|---|---|---|
| R1 | List nav clamp/wrap | **11** | `input.rs:531-552, 606-611, 1898-1918, 2085-2103, 2289-2311, 1948-1953, 2372-2377, 2404-2410, 2457-2463, 2503-2510, 2532-2537` | `fn move_index(sel:&mut usize, len:usize, down:bool)->bool` |
| R2 | Bool confirm-dialog boilerplate (~19 ln each) | **6** | `input.rs:2554, 2640, 2661, 2681, 2762, 2782` | generic `handle_bool_confirm_dialog(...)` |
| R3 | Checkbox rendering — 3 parallel impls | 3 | `components/checkbox.rs:39` (5 uses); free fn `checkbox_line` `render.rs:6882` (8 uses, "audit03 Phase 6"); inline `"[x]"/"[ ]"` `render.rs:4542` | one `components::Checkbox` |
| R4 | Button rendering — 2 impls, different visuals | 2 | `components::Button` (rounded border, 24 uses); `button_row` `render.rs:6904` (plain `[ Save ]`, hardcoded labels, 1 use at `:5717`) | `components::Button` |
| R5 | Popup list-panel border | **8** | `render.rs:2364, 2537, 2721, 2900, 3075, 3186, 3348, 6354` | `render_overlay_list_below_input(..) -> Rect` |
| R6 | Hint/footer bar spans | 4 used vs **11 inline** | helper `edit_macro_hints` `render.rs:6144`; inline at `render.rs:2333, 2486, 2563, 2621, 2804, 2982, 3109, 3291, 5359, 6274` | lift helper to general |
| R7 | 3-way focus-cycle state machine | 3 | `input.rs:2596-2606, 2716-2739, 1957-1968` | pattern **already solved** at `input.rs:2984-3006` (`NameNewAgentFocus`) |
| R8 | "enter interactive or reconnect" logic | **5** | `input.rs:578-596, 653-661, 5486-5495, 5502-5511, 5528-5536` | `fn enter_agent_or_reconnect(&mut self) -> Result<bool>` |
| R9 | String truncation — 4 impls, 3 weaker | 4 | width-aware+tested: `truncate_status_text` `render.rs:7156`; char-count-only: `render.rs:2311, 3229, 6338` | route all through `truncate_status_text` |
| R10 | `App` test fixture hand-built | 2 | `input.rs:6238 test_app`, `sessions.rs:2847 test_app_with_sessions` | one `App::for_test()` builder |
| R11 | Key-vs-mouse handler pairs | 2 pairs | `input.rs:1826`≡`5062`; `input.rs:1878`≡`5111` | shared helpers |
| R12 | `sanitise_handle` | 2 | `inject_runtime.rs:157`; `peer.rs` (used `:328,363,390,646`) | one home in `crate::amq_inject` |
| R13 | ~200 lines of pure protocol fns in app layer | — | `inject_runtime.rs:96,157,163,199,1185,1205,1217,1228,1252,1275,1297` — all `self`-free, fully unit-tested | move to `src/amq_inject/protocol.rs` |

R1+R2+R5+R6+R8 alone remove ~500 lines and 31 independent places an off-by-one or
styling drift can hide.

## Coherence defects

**C1 — audit02 P1-V delivered indirection, not encapsulation (headline).**
Plan at `docs/audits/audit02/audit02.md:174` specifies
`App { ui, runtime, git, remote, config, theme }` and: *"Each submodule's `impl App`
block becomes `impl GitState` / `impl UiState` etc. with thin `App` shims that
delegate."* Reality: `RemoteState` **does not exist**; **zero `impl` blocks on any
sub-struct**; all 72 sub-struct fields are `pub(crate)` inside `pub(crate)` structs.
All 373 methods still reach every field. Field count went 120 → 117.
**Step 1 landed, step 2 never started.**

**C2 — CLAUDE.md `state/` paragraph is stale in three places** (`CLAUDE.md:77`,
echoed in `state/mod.rs:6-12`):
- *"PTY map"* — gone. `providers` map deleted (`state/runtime.rs:25-28`); agent PTYs
  now live inside `SessionState::Live`/`Detached`, found via `App::find_pty_handle`
  (`mod.rs:3049`). Only companion-terminal PTYs remain in `RuntimeState`.
- *"scroll offsets"* in `UiState` — only `help_scroll`. `left_scroll_offset`
  (`mod.rs:68`) and `prev_scrollback_offset` (`mod.rs:90`) are loose on `App`;
  diff scroll is inside `CenterMode::Diff`; prompt scrolls inside `PromptState`.
- *"modal stack"* — there is no stack. `ui.prompt` is a single slot
  (`state/ui.rs:16`). `close_top_overlay` (`mod.rs:2077-2112`) is a hardcoded
  4-level ladder (fullscreen → prompt → help → diff) that **excludes the macro bar**.
  Nesting is faked per-variant: `ConfirmKillRunning { previous }` clones prior state
  back (`input.rs:2559`); `EditMacros { pending_delete }` embeds a sub-confirm.

Also inconsistent: `create_agent_in_flight`/`resource_stats_in_flight` on `App`
(`mod.rs:87-88`) while 7 identical in-flight markers sit in `GitState`;
`last_pty_activity` (PTY state) and `collapsed_projects` (UI state) sit in `GitState`.

**C3 — input and render disagree on exhaustiveness for the same enum.**
`render_prompt` (`render.rs:2279`) is an exhaustive `match`, 25 arms, no `_`.
`handle_prompt_key` (`input.rs:1773`) is a cascade of **39 `if let
PromptState::…` checks** ending in bare `Ok(false)` (`input.rs:2871`).
**Adding a `PromptState` variant breaks the build in render and silently produces
a dead, unresponsive modal in input.**

**C4 — `PromptState` (25) and `OverlayMouseLayout` (21) are hand-synced parallel
enums.** `mod.rs:480` and `mod.rs:955`. Six modals have no mouse layout and are
keyboard-only: `WatchRules`, `EditMacros`, `ConfirmSharedWriter`, `OrphanWorktrees`,
`ConfirmRemoveOrphanWorktree`, `DebugInput`. Adding a modal requires edits in 4+
unlinked places.

**C5 — the command palette is a second, string-typed dispatch table — with a live bug.**
`BINDING_DEFS` carries `palette: Option<PaletteEntry>` (`keybindings.rs:184`), but
selecting a row passes `binding.palette_name` — a `&'static str` — to
`execute_command` (`mod.rs:2278-2452`), a hand-written `match` over ~41 string
literals with `other => set_error("Unknown command")` (`mod.rs:2431`). Nothing links
the two tables at compile time.

> **Shipping bug: `Action::DeleteTerminal` is dead.** Enum entry
> (`keybindings.rs:28`), config name (`:208`), help text (`:302`), section (`:402`),
> palette entry `"delete-terminal"` (`:792-801`) with `default_keys: &[]` and
> `scopes: &[]` — so the palette is its **only** surface. There is no
> `execute_command` arm and no handler anywhere in `src/app/`. Selecting
> *"Delete the selected companion terminal"* shows `Unknown command:
> "delete-terminal"`. Zero test coverage.

**C6 — the entire git workflow is unreachable from the palette.** 80 `BindingDef`s,
42 with palette entries. `CommitChanges`, `PushToRemote`, `PullFromRemote`,
`DiscardChanges`, `GenerateCommitMessage`, `StageUnstage`, `EngageCommitInput` all
have `palette: None` (`keybindings.rs:940-1010`). Contradicts *"Prefer
command-palette actions over adding many more global hotkeys."*

**C7 — Space tenet violated in exactly one dialog.** 14 of 15 button dialogs handle
Space correctly. In `KillRunning`, the `Action::Confirm` arm **is** focus-aware
(`input.rs:2130-2143`), but the `Action::ToggleMarked` arm (`input.rs:2126-2128`) —
which `space` is bound to in `BindingScope::RuntimeKill` (`keybindings.rs:1311`) —
calls `toggle_hovered_kill_running_selection()` unconditionally. Tab to a footer
button, press Space, and it toggles a list row instead. A test covers Tab reaching
the button (`input.rs:7029`) but not Space firing it. ~3-line fix.

**C8 — the macro bar bypasses `PromptState`.** Separate `Option<MacroBarState>` on
`UiState` (`state/ui.rs:35`), checked *before* the prompt check (`input.rs:285`),
with hardcoded `KeyCode::Esc => self.close_macro_bar()` (`input.rs:892-894`) instead
of `Action::CloseOverlay`, and absent from `close_top_overlay`. Rebinding
overlay-close away from Esc closes every modal except this one.

**C9 — tick-count tenet violated twice, with correct code adjacent.**
- `workers.rs:1042` — `if self.tick_count.is_multiple_of(20)` gating a "~every 2
  seconds" poll. **The very next block (`workers.rs:1050-1056`) does the same job
  correctly** with `last_refresh.elapsed() >= Duration::from_secs(2)`.
- `mod.rs:2210-2215` — `const NUDGE_DURATION_TICKS: u64 = 15; // ~1.5s at 100ms/tick`.

Compliant elsewhere: `spinner_frame_index` `mod.rs:2128`; all AMQ/watch timers use `Instant`.

**C10 — a stale-result race the codebase already knows about and fixed once.**
`WorkerEvent::ReloadChangedFilesReady` (`workers.rs:546-555`) carries a `worktree`
and guards on it, commented: *"Discard out-of-order replies…"*.
`WorkerEvent::ChangedFilesReady` (`mod.rs:1181-1184`) — same job, ~470 lines away in
the same function — **carries no worktree, no session id, no generation counter**,
and its arm (`workers.rs:77-81`) assigns unconditionally. The poller reads
`watched_worktree` (`workers.rs:1705`) then spends three `git` subprocess round-trips;
a selection change inside that window lands the old session's file list on the new
session's pane, uncorrected for 2-10s. Fix: add `worktree: PathBuf`, copy the sibling guard.

**C11 — blocking work on the UI thread** (violates `CLAUDE.md:62`). Highest severity first:

| Site | Reached from | Blocks |
|---|---|---|
| `peer.rs:730` ← `sessions.rs:1192` | `drain_events` (`workers.rs:354`) | **`flock(LockExclusive)` — blocking cross-process lock, no timeout, no `_Nonblock`.** Another AMQ process holding it freezes the TUI indefinitely. |
| `mod.rs:3538-3554` ← `tick_watch_engines` (`mod.rs:3494`) | **run loop, ~10 Hz** | up to 3 `fs::read_dir` scans *per watched session* (`amq_activity.rs:41,53,62`) |
| `workers.rs:1042` → `pty.rs:657` → `pty.rs:798` | `drain_events` | on **macOS** forks `ps -p <pid> -o comm=` per companion terminal every ~2 s |
| `mod.rs:3283, 3309` | `drain_events` epilogue | `PtyClient::drop` → `recv_timeout(250ms)` + `recv_timeout(1s)` + `join` — **up to 1.25 s stall per exiting session** (`pty.rs:694-710`) |
| `git.rs:358` ← `sessions.rs:245` | `drain_events` (`workers.rs:27`) | `git rev-parse --git-path info/exclude` + symlink/dir writes |
| `input.rs:3279` → `git.rs:188,199` | `handle_key` | 2 × `git rev-parse` while the user presses Enter |
| `inject_runtime.rs:367-476` | `drain_events` (`workers.rs:790`) | up to 32 file reads + stats + renames per drain |
| `workers.rs:179, 260, 1797` | `drain_events` | sqlite `upsert_*` **inside `for` loops**, unbounded in N |
| `workers.rs:1849` → `sessions.rs:632` | `drain_events` (`:774`) | `fork`+`exec` of a provider CLI |

*Not* a defect: `mod.rs:1998` fork-on-main is deliberate and documented (macOS
forbids forking from a worker thread).

**C12 — the file grew after being flagged.** `audit02.md` P2-18 recorded `input.rs`
at 10,506 LOC and said *"Extract palette, prompt-state-machine, mouse subsystems."*
It is now 13,266 and none of the three were extracted. `CLAUDE.md:135` sets a
~200-line guideline per concern; `render_prompt` alone is 15× that.

**C13 — cosmetic:** `mod` declarations sit at `mod.rs:1406-1413`, after 1,278 lines
of type definitions.

### Tenets verified COMPLIANT — do not spend refactor budget here

- **Theme**: `Color::` in prod render code = 6, all `Color::Reset` sentinels or the
  xterm256/grayscale converters.
- **Git Command Safety**: `git.rs:696,748,775` use `--porcelain=v1 -z` /
  `--numstat -z`; `symbolic-ref --quiet --short HEAD` at `:59`;
  `-c color.diff=false` at `:970`.
- **Byte-slicing**: no panic risk found — `render.rs:6336` uses
  `char_indices().nth()`; `render.rs:6299`/`6941` rely on `TextInput`'s enforced
  boundary invariant at `text_input.rs:479-506`.
- **Keybinding labels** (this domain only): one violation, `input.rs:4461`
  hardcodes "Space" for `Action::ToggleMarked`.
- **Interactive-mode suppression and scroll gating**: `keybindings.rs:1995`
  correctly excludes `ScrollPage*` from the conditional set.

## Risk register

**Safe to move.** Inline `#[cfg(test)]` tests travel with their code, so a pure file
split preserves them for free. `input.rs` (236 behavioral tests driving
`handle_key`/`handle_mouse` through a real `App`), `sessions.rs` (53),
`text_input.rs` (93, self-contained), `workers.rs` (16), and the 24 pure free
helpers in `render.rs` are all defensible moves.

**Would move blind.**
- **`render.rs` is the danger.** 7,178 production lines, 43 tests, and **every one
  covers a pure helper** — `centered_rect_exact`, `capitalize`, `format_bytes`,
  `truncate_status_text`, `to_grayscale`, `xterm256_*`, `wrapped_line_count`,
  `pty_cell_colors`. **Zero tests call any `render_*` method.** `render.rs` uses
  `TestBackend` **zero** times, while `input.rs` uses it 7 times. `render_prompt`
  (3,109 lines, 25 modals) has no coverage whatsoever.
- `mod.rs`: 13 tests for 4,158 lines. The run loop, `execute_command`, and
  `close_top_overlay`'s ladder are untested from within `mod.rs`
  (`close_top_overlay` is incidentally covered from `input.rs:9915`).

**Specific breakage vectors.**
1. **Two hand-rolled `App` literals** — `input.rs:6374` and `sessions.rs:2955`, each
   enumerating all 48 fields plus full `UiState`/`RuntimeState`/`GitState` literals.
   Any field move breaks both. Unify into one `App::for_test()` builder **before**
   touching fields.
2. `last_snapshot_id` (`mod.rs:117`) is the only private field; moving
   `refresh_snapshot_buf` out of `mod.rs` requires widening it to `pub(crate)`.
3. Visibility churn: 373 methods are currently siblings inside one crate module tree.
   A directory split is mechanical (9 `impl App` blocks already exist) but each moved
   method needs its `pub(crate)`/private status re-checked.
4. `render_prompt`'s 25 arms each end by writing `self.ui.overlay_layout.active = …`.
   That write is the sole contract with `input.rs`'s mouse hit-testing
   (`input.rs:3823` onward). Dropping or reordering one during extraction silently
   kills mouse support for that modal — **with no test to catch it**.
5. The `if let` cascade in `handle_prompt_key` has no exhaustiveness check (C3), so
   an extraction that drops a block compiles cleanly.

**Tooling available.** CI gates: `cargo fmt --check` (`pr.yml:30`),
`cargo clippy --all-targets --all-features -- -D warnings` (`pr.yml:63`),
`cargo test --all-features` (`pr.yml:88`, `test.yml:40`). Dev-dependencies are only
`filetime` and `serial_test` — **no `insta`, `proptest`, or `rstest`**.
`ratatui::backend::TestBackend` is already in use (7 sites in `input.rs`), so
golden-buffer tests for `render_prompt` can be added with **zero new dependencies**.
None of the 13 files in `tests/` touch `src/app/` (`dux::app` appears 0 times), even
though `pub mod app` (`lib.rs:14`) and `pub struct App` make it reachable.

**Recommended order.** (1) Unify the test fixture. (2) Add `TestBackend` snapshot
tests for all 25 `render_prompt` arms — the missing safety net, cheap given
`test_app()` exists. (3) Finish the field migration into `UiState`/`InputState`.
(4) Split files. (5) Then `impl UiState`-style encapsulation, which is the actual C1
fix and the only step that reduces coupling rather than relocating it.

## Open questions

1. Was `RemoteState` deliberately abandoned, or still intended? `gh_status`, `pr_*`
   and the AMQ/orchestrator maps (~19 of `RuntimeState`'s 33 fields) are exactly the
   cluster the plan earmarked for it.
2. Is `Action::DeleteTerminal` (C5) an unfinished feature or a regression from a
   removed handler? Worth a `git log -S "delete-terminal"`.
3. Is the git-workflow palette gap (C6) deliberate — staging/commit being
   pane-contextual — or an oversight?
4. Does the `should_suppress_auto_clear` read_dir storm (C11) actually manifest? No
   agent measured it under realistic multi-agent load.
5. Is the `ChangedFilesReady` poller variant (C10) still needed at all, given
   `ReloadChangedFilesReady` covers selection-driven refresh? Deleting the older
   variant may be simpler than guarding it.
6. Should `inject_runtime.rs` live in `src/app/` at all? ~480 lines are genuine
   UI-tick glue; ~200 are pure protocol code with no `App` dependency, plus 449 lines
   of tests that only exercise the pure half.
7. No sub-agent measured which `App` fields each file actually mutates. That fan-out
   matrix is what would tell you whether an `impl UiState` split is a 2-day or a
   2-week job.

---

## Verification notes (main agent, first-hand)

Two of parent A's claims were re-checked directly. One holds as stated; one was
overstated and is corrected here. Plans must use the corrected version.

### C5 `Action::DeleteTerminal` — CORRECTED, narrower than reported

Parent A reported the delete-terminal *feature* as dead ("selecting it shows
`Unknown command`"). That overstates it. Verified reality:

- The **capability is live and tested.** `confirm_delete_selected_terminal`
  (`sessions.rs:1322`) → `PromptState::ConfirmDeleteTerminal` (`sessions.rs:1328`)
  → `resolve_confirm_delete_terminal` (`input.rs:4684`) → `do_delete_terminal`
  (`sessions.rs:1336`). Full render arm (`render.rs:3776-3855`), full mouse layout
  (`input.rs:3955`), and three passing tests (`input.rs:11887`, `:11911`, `:11947`).
- **It is reached via `Action::DeleteSession`**, not `Action::DeleteTerminal` —
  `input.rs:633`: `Action::DeleteSession => self.confirm_delete_selected_terminal()?`.
  This matches the commit that introduced it: `3a923de` *"Add Ctrl+D to delete
  companion terminals from the terminals pane (#162)"*, 2026-04-10.
- **What is actually broken:** `Action::DeleteTerminal` is a vestigial enum variant
  with `default_keys: &[]` and `scopes: &[]` (`keybindings.rs:791-801`), so the
  command palette is its only surface, and `execute_command` (`mod.rs:2278-2452`)
  has no `"delete-terminal"` arm. `mod.rs:2297-2298` handles `"show-terminal"` and
  `"new-terminal"` but not this one. So **the palette row is broken while the
  keybinding path works** — a narrower user-visible bug than reported, but real.

**Second-order finding (new):** `Action::DeleteSession` is overloaded — it deletes an
agent session in one context and a companion terminal in another. That context
sensitivity is invisible in the help overlay, which renders one description per
action. Worth folding into the palette/action unification work rather than patching
`execute_command` alone.

### C10 `ChangedFilesReady` stale-result race — CONFIRMED as reported

Verified verbatim:
- `WorkerEvent::ChangedFilesReady { staged, unstaged }` (`mod.rs:1181-1184`) carries
  **no** worktree, session id, or generation counter.
- Its drain arm (`workers.rs:77-81`) assigns `self.git.staged_files` /
  `self.git.unstaged_files` unconditionally, then `clamp_files_cursor()`.
- Its sibling `ReloadChangedFilesReady { worktree, result }` (`workers.rs:546-555`)
  carries a `worktree` and guards on it, with the comment *"Discard out-of-order
  replies: by the time the worker returned, the user may have switched to a
  different session. The currently-selected worktree wins."*

The asymmetry is real and the fix is the one parent A proposed.
