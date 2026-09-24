# Phase 10 — Live defect fixes

**Track:** C (correctness & resources) · **Parallel-safe with:** 08, 09, 11
**Depends on:** nothing (item 3 is easier after 06) · **Blocks:** nothing

## Goal

Fix the user-visible defects the research surfaced. Every one is reproducible today;
none requires the decomposition to have happened. They are grouped here so they ship
in weeks, not behind a multi-month refactor.

## Evidence and work items

### D1 — Stale-result race puts one session's files on another's pane

**Confirmed verbatim at the top level.** `WorkerEvent::ChangedFilesReady`
(`mod.rs:1181-1184`) carries **no** worktree, session id, or generation counter, and
its drain arm (`workers.rs:77-81`) assigns `self.git.staged_files` /
`unstaged_files` unconditionally. Its sibling `ReloadChangedFilesReady`
(`workers.rs:546-555`) carries a `worktree` and guards on it, commented: *"Discard
out-of-order replies: by the time the worker returned, the user may have switched to a
different session."*

The poller reads `watched_worktree` (`workers.rs:1705`) then spends three `git`
subprocess round-trips; a selection change inside that window lands the old session's
file list on the new session's pane, **uncorrected for 2–10 s**.

**Fix:** add `worktree: PathBuf` to the variant and copy the sibling's guard.
**First consider deleting the variant entirely** — `ReloadChangedFilesReady` already
covers selection-driven refresh, and removing the older poller may be simpler than
guarding it. Decide with evidence, then test the race explicitly.

### D2 — Space on a focused KillRunning button toggles a list row

14 of 15 button dialogs handle Space correctly. In `KillRunning`, the
`Action::Confirm` arm **is** focus-aware (`input.rs:2130-2143`), but the
`Action::ToggleMarked` arm (`input.rs:2126-2128`) — which `space` is bound to in
`BindingScope::RuntimeKill` (`keybindings.rs:1311`) — calls
`toggle_hovered_kill_running_selection()` unconditionally. Tab to a footer button,
press Space, and it toggles a list row instead of pressing the button.

This breaks a hardcoded accessibility tenet: *"Space activates the focused button in
confirmation dialogs… every modal that presents buttons must treat Space as equivalent
to Enter."* A test covers Tab reaching the button (`input.rs:7029`) but **not Space
firing it**. ~3-line fix: mirror the `Confirm` arm's focus match. Add the missing test.

### D3 — The `delete-terminal` palette row is broken

**Corrected from the original research claim** — the capability is live and tested;
only the palette entry is dead.

`confirm_delete_selected_terminal` (`sessions.rs:1322`) → `PromptState::ConfirmDeleteTerminal`
→ `resolve_confirm_delete_terminal` (`input.rs:4684`) → `do_delete_terminal`
(`sessions.rs:1336`) works, with a render arm (`render.rs:3776-3855`), a mouse layout
(`input.rs:3955`), and three passing tests (`input.rs:11887, 11911, 11947`). It is
reached via **`Action::DeleteSession`** (`input.rs:633`), matching the commit that
added it (`3a923de`, *"Add Ctrl+D to delete companion terminals from the terminals
pane"*).

But `Action::DeleteTerminal` (`keybindings.rs:791-801`) has `default_keys: &[]` and
`scopes: &[]`, so the palette is its **only** surface — and `execute_command`
(`mod.rs:2278-2452`) has no `"delete-terminal"` arm (`:2297-2298` handles
`"show-terminal"` and `"new-terminal"`). Selecting *"Delete the selected companion
terminal"* shows `Unknown command: "delete-terminal"`.

**Fix:** add the `execute_command` arm. **Second-order finding to address:**
`Action::DeleteSession` is overloaded — it deletes an agent session in one context and
a companion terminal in another, and the help overlay renders one description per
action, so that context sensitivity is invisible. Make the help text context-aware or
split the action.

### D4 — `right_width_pct` silently ignores documented values

**Confirmed at the top level.** `MAX_RIGHT_WIDTH_PCT = 50` (`app/input.rs:10`) while
`config.rs:1508` documents the range as `(5-80)`. A value of 60–80 loads fine and then
silently snaps to 50 on the first resize keypress. The config file is meant to *be* the
documentation, so this falsifies its own tenet. **Fix the clamp to match the docs**,
and add a test asserting the constant and the comment agree.

### D5 — `dux config reset --all` destroys worktrees with no confirmation

`run_reset` (`cli.rs:360-388`) calls `reset_agent_data` on `--all`, which removes
worktrees via `git worktree remove --force` (`:710-720`), falls back to
`fs::remove_dir_all` (`:721-728`), and force-deletes branches with `git branch -D`
(`:731-737`). **No interactive prompt and no `--yes` flag on this path.**

The structural guards are good — `whole_workspace_target_is_within` (`:695`),
`guard_whole_workspace_removal` (`:702`), a shared-workspace skip (`:691`), and a
complete read-only inventory before the first mutation (`:565-598`). None is *user*
confirmation.

The inconsistency is stark: `dux session purge` requires **both** `--hard` and a typed
`PURGE <branch>` (`cli.rs:218-231`), and `config regenerate` requires `--yes` — yet the
most destructive command requires neither. Worktrees are explicitly user data under
`CLAUDE.md`. **Fix:** require a typed confirmation naming what will be destroyed, plus
a `--yes` escape for scripts, matching the purge pattern.

### D6 — Five hardcoded keybinding labels

All have `self.bindings` in scope. A naive `Ctrl-`/`^X` search finds only B1; the other
four use bare `Enter`/`Space`/`Esc`.

| | Site | Literal | Action |
|---|---|---|---|
| B1 | `config.rs:1578` | "…the palette (Ctrl-p) for an interactive picker." | `OpenPalette` |
| B2 | `sessions.rs:2220` | "…press Enter to open the selected worktree." | `Confirm` |
| B3 | `sessions.rs:2416` | "…Press Space to select one or more runtimes first." | `ToggleMarked` |
| B4 | `sessions.rs:2433` | "…press Enter to confirm, or Esc to keep…" | `Confirm` + `CloseOverlay` |
| B5 | `input.rs:4461` | "…Press Space to mark the highlighted row." | `ToggleMarked` |

Reference implementation: `WELCOME_TIPS` (`render.rs:58-220`), where every tip is
`fn(&RuntimeBindings) -> String`. A regression test for this class already exists —
`config_comment_uses_dynamic_keybinding_label` (`config.rs:4148-4160`); extend it to
all five. B1 should also switch `CommentSource::Static` → `Dynamic`, which
`commit_prompt` already does correctly at `config.rs:1428`.

### D7 — A label that is confidently wrong

`render.rs:6576` renders the Resource Monitor's close hint from
`label_for(Action::CloseOverlay)` — dynamic and correct-looking — while the handler at
**`input.rs:1787`** is a bare `if key.code == KeyCode::Esc` that never consults
`lookup`. Rebind `close_overlay` and the footer advertises the new key while only `Esc`
works. **Worse than a stale hardcode, because the UI is confidently wrong.**

`input.rs:1834-1840` is the identical hardcode but carries the rationale
*"Esc always closes — hardcoded so a broken binding can't trap the user."* That design
is sound. **Fix:** adopt the same rationale at `:1787` **and** make the label static to
match, or route the handler through `lookup`. Pick one; do not leave them disagreeing.

### D8 — Three modals cannot be rebound at all

No `BindingScope` covers them, violating *"All settings are configurable"*:
macro editor (`input.rs:3537-3709`, hints `render.rs:5878-5996`), macro bar
(`input.rs:891-956`, hint `render.rs:6275`), help-overlay Space-scroll
(`input.rs:326-330`, label `render.rs:2260`). The Resource Monitor's row-expand
(`input.rs:1812`) is a fourth with no `Action`.

**Fix:** give each a `BindingScope` and real `Action`s. Where a hardcode is deliberate
(the help-overlay Space avoids colliding with `ToggleProject`/`StageUnstage`), keep it
but **document the rationale inline**, as `input.rs:1835` already does — the
inconsistency is that three of four are undocumented.

### D9 — The macro bar escapes the modal contract

It is a separate `Option<MacroBarState>` on `UiState` (`state/ui.rs:35`), checked
*before* the prompt check (`input.rs:285`), with hardcoded
`KeyCode::Esc => self.close_macro_bar()` (`input.rs:892-894`) instead of
`Action::CloseOverlay`, and it is **absent from `close_top_overlay`**
(`mod.rs:2077-2112`). Rebind overlay-close away from Esc and every modal closes except
this one. Violates *"Route new modal UI through `PromptState` so Esc keeps working
uniformly."* **Fix:** route it through `PromptState` and add it to the ladder.

### D10 — The entire git workflow is unreachable from the palette

80 `BindingDef`s, 42 with palette entries. `CommitChanges`, `PushToRemote`,
`PullFromRemote`, `DiscardChanges`, `GenerateCommitMessage`, `StageUnstage`,
`EngageCommitInput` all have `palette: None` (`keybindings.rs:940-1010`), contradicting
*"Prefer command-palette actions over adding many more global hotkeys."*

**Decide explicitly**: either these are pane-contextual and meaningless without a
focused file — in which case document that reasoning in `keybindings.rs` — or they are
an oversight and get palette entries. Do not leave it undecided.

### D11 — Purge dry-run reports failure

`purge.rs:47-51` claims *"Every step honors `dry_run`"* and *"the integration tests
assert this explicitly per category."* **Both clauses are false.**
`PurgeItem::SharedProviderHistory` (`purge.rs:675-688`) **never reads `dry_run`** —
it returns `Skipped` or `Error` unconditionally, so a dry run of a shared-workspace
plan returns `PurgeOutcome::Error` with `had_errors() == true`. **A rehearsal reports
failure**, eroding trust in the safety check before an irreversible operation.

And there is exactly **one** `execute(..., true)` call in the whole suite
(`tests/purge_integration.rs:366`), whose coverage of 5 of 6 variants is *incidental* —
a blanket `all(matches DryRun)` that happens to hit them because the fixture session is
non-shared. `SharedProviderHistory` is uncovered; all four shared-plan tests pass
`dry_run = false`. **Fix the code, fix the comment, and add per-category dry-run tests.**

### D12 — Editor selection is a closed table

`editor.rs:33-61` (`EDITOR_SPECS`) is a closed 4-entry table with `--new-window`
hardcoded per kind (`:125-133`), and `$EDITOR`/`$VISUAL` are **never consulted**.
Adding an editor is a code change — the same defect class as the provider tenet
(Phase 11), one layer over. **Fix:** make editors config-driven and honour
`$EDITOR`/`$VISUAL` as a fallback.

### D13 — Diff syntax theme does not follow the dux theme

`diff.rs:77` hardcodes `"base16-ocean.dark"`. Make it a config key
(`[ui] syntax_theme`) and default it to something derived from the active dux theme.

## Acceptance criteria

- [ ] D1: race fixed (guard added or variant deleted) with an explicit regression test.
- [ ] D2: Space fires the focused button in `KillRunning`; test added.
- [ ] D3: palette row works; `DeleteSession` overload documented or split.
- [ ] D4: clamp matches the documented 5–80; test asserts constant and comment agree.
- [ ] D5: `reset --all` requires typed confirmation plus a `--yes` escape; tested.
- [ ] D6: all five labels dynamic; `config_comment_uses_dynamic_keybinding_label`
      extended to cover them.
- [ ] D7: label and handler agree at `render.rs:6576` / `input.rs:1787`.
- [ ] D8: three modals rebindable, or hardcodes documented inline with rationale.
- [ ] D9: macro bar routed through `PromptState` and present in `close_top_overlay`.
- [ ] D10: git actions have palette entries, or the omission is documented in code.
- [ ] D11: `SharedProviderHistory` honours `dry_run`; comment corrected; per-category
      dry-run tests added.
- [ ] D12: editors config-driven; `$EDITOR`/`$VISUAL` honoured.
- [ ] D13: `[ui] syntax_theme` config key exists and is documented.
- [ ] `cargo test --all-features` green; no snapshot churn beyond the intended fixes.

## Validation

```bash
cargo test --all-features
cargo clippy --all-targets --all-features -- -D warnings

# D1: switch sessions rapidly during a git refresh; files pane must never show
#     another session's changes
# D2: open Kill Running, Tab to a footer button, press Space -> button fires
# D3: palette -> "Delete the selected companion terminal" -> confirm dialog appears
# D4: set right_width_pct = 70, resize once, confirm it is not snapped to 50
# D5: dux config reset --all  -> must refuse without typed confirmation
```

## Risks

| Risk | Mitigation |
|---|---|
| D5's confirmation breaks scripted/CI use | Provide `--yes` from the start, matching `config regenerate` |
| D9's `PromptState` migration changes macro-bar behaviour | It is checked *before* the prompt at `input.rs:285` for a reason; preserve precedence explicitly and cover with a Phase 06 snapshot |
| D1's deletion option removes a needed refresh path | Trace both producers before deleting; if unsure, guard rather than delete |
| D8 changes default keys users rely on | Introduce `Action`s with the **current** keys as defaults; rebindability is the change, not the bindings |

## References

- `artifacts/research-app-monoliths.md` §C5–C10, §Verification notes
- `artifacts/research-config-keys-theme-cli.md` §3.2–3.6, §3.5 (H1, H22)
- `artifacts/research-runtime-memory.md` §D-PU1
