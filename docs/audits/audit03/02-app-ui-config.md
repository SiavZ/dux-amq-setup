# Application, UI, and configuration

This subsystem covers input/render paths, session settings, runtime keybindings, defaulting, configuration comparison, and persistence. Baseline is `3d52074`.

## P0-08 — Unicode macro text can panic the normal render path

**Impact.** Opening the macro list at an ordinary narrow terminal width can panic when a user-authored macro contains multibyte UTF-8.

**Baseline evidence.** `3d52074 — src/app/render.rs:5658-5685 — macro list item renderer`: `text_preview.len()` counts bytes, the available limit is a terminal-column count, and line 5678 slices `&text_preview[..max_len - 1]`. If the width boundary lands inside an emoji, CJK, or accented code point, Rust panics. `3d52074 — CLAUDE.md:142 — UTF-8 rendering constraint` explicitly prohibits this exact `.len()`/`[..n]` pattern because user-visible strings are multibyte.

**Adversarial verification.** The second macro preview at `src/app/render.rs:6042-6053` correctly derives a byte boundary with `char_indices`, proving no upstream ASCII invariant exists and that a local safe pattern is available. Macro names/text are user configuration, not sanitized to ASCII. `saturating_sub` avoids integer underflow but cannot make the byte index a character boundary. The failure is a normal configured-input render panic and remains P0.

**Secondary verified exposure.** `src/app/render.rs:4355-4357,4589-4591` also uses `split_at(1)` on editable/restored strings. Normal key entry is usually ASCII-mapped, so that path does not independently meet P0, but it violates the same invariant and should be covered by the same Unicode regression pass.

**Required proof direction.** Truncate by terminal display width/character boundary and test emoji plus double-width CJK at widths immediately before, within, and after a multibyte glyph.

## P1-16 — Session settings mutate live memory before persistence succeeds

**Impact.** A database failure leaves session settings/title changed in live memory even though the modal reports that they remain unsaved and asks the user to retry.

**Baseline evidence.** `3d52074 — src/app/sessions.rs:2241-2311 — save_session_settings`: lines 2288-2294 mutate the live `AgentSession`; only afterward do lines 2296-2303 call `upsert_session`. On error, lines 2304-2310 leave the modal open and claim “Settings remain unsaved,” but never restore the prior in-memory values. Success-only tests begin at `src/app/sessions.rs:3546`; no injected storage failure asserts rollback.

**Adversarial verification.** Runtime watch hooks are applied only after successful persistence, which limits one side effect but makes the split state worse: display/model fields are new while hooks/database are old. The draft still contains the desired values, so persisting first requires no speculative mutation. Concrete failure ordering violates explicit-failure/state-coherence expectations. P1.

## P1-20 — Generated and deserialized UI defaults disagree

**Impact.** Omitting `[ui]` (or deserializing a partial/default UI value) yields a different pane layout than generating a fresh `Config`, so equivalent “default” configurations behave differently.

**Baseline evidence.** `3d52074 — src/config.rs:686-720 — Config::default` constructs `UiConfig` with left/right widths 20/23. `3d52074 — src/config.rs:827-843 — UiConfig::default`, used by `#[serde(default)]`, returns 17/19. Other fields match. The canonical renderer and config-diff paths use `Config::default`, while Serde defaulting can use `UiConfig::default`.

**Adversarial verification.** This is not an intentional platform split: both implementations are unconditional and adjacent in one module. No migration normalizes one pair to the other. The disagreement affects visible layout and config comparison on supported inputs. P1.

## P1-21 — Keybinding conflict validation disagrees with runtime matching

**Impact.** Validation accepts bindings that runtime lookup silently shadows. For example, same-scope `ctrl-x` and `ctrl-alt-x` pass conflict detection, while an incoming `ctrl-alt-x` can match the earlier `ctrl-x` binding.

**Baseline evidence.** `3d52074 — src/keybindings.rs:1695-1718 — RuntimeBindings::lookup`: a modifier binding matches when incoming modifiers *contain* the configured set. `3d52074 — src/keybindings.rs:1853-1868 — keys_conflict`: two non-plain bindings conflict only when modifier sets are exactly equal. `detect_conflicts` at lines 1891-1935 relies on that narrower predicate and claims to prevent declaration-order shadowing.

**Adversarial verification.** Normalization does not erase Alt from `ctrl-alt-x`. Plain-vs-modified is correctly distinct, but subset-related modified bindings are not. Existing tests cover identical, different-scope, and plain-vs-modified cases, not modifier subsets. Because lookup uses `.find`, order chooses the winner exactly as the validator says should be impossible. P1.

## P1-23 — The main UI path still performs blocking filesystem and Git work

**Impact.** Slow filesystems, Git locks, large diffs, or network-backed directories can freeze input/rendering despite the project's categorical worker-thread rule.

**Baseline evidence.** `3d52074 — CLAUDE.md:60-63 — Important Constraints` says all periodic or potentially blocking Git/file I/O must use workers, including fast Git commands. Direct event-handler paths remain:

- `src/app/input.rs:977-1000 — toggle_stage_selected_file` synchronously calls stage/unstage and reload;
- `src/app/input.rs:2037-2087 — project path autocomplete` synchronously calls `is_dir`, `read_dir`, `file_type`, collects, and sorts;
- `src/app/input.rs:4583-4604 — resolve_confirm_discard_file` synchronously performs destructive Git/file work;
- `src/app/sessions.rs:1682-1737 — open_diff_for_selected_file / refresh_current_diff` synchronously invokes Git/object reads, worktree reads, syntax processing, and diff rendering;
- configuration save calls from input/session handlers synchronously read, parse, and rewrite the config.

**Adversarial verification.** The changed-file poller and several lifecycle commands do use workers, so this is not a claim that all UI work is blocking. The listed functions are invoked directly from input/modal dispatch and contain no channel handoff. Each has a credible stall source independent of file size (Git index locks and filesystem latency). Under the frozen bar and explicit project tenet this is P1, not inherited P0 from audit02.

## P1-26 — Config diff omits behavior-changing sections and arguments

**Impact.** `dux config diff --summary` can print “config matches defaults” while active behavior differs, defeating an operator diagnostic used before regeneration or support collection.

**Baseline evidence.** `3d52074 — src/cli.rs:367-524 — run_diff_summary` compares only selected defaults/logging/UI/editor/terminal/key/provider/project/macro fields. It omits full behavior-changing sections including storage, limits, AMQ, and auto-resume, plus several defaults/UI values. `3d52074 — src/cli.rs:633-653 — diff_providers` compares provider commands but omits provider args and watch configuration. Yet the empty `changes` branch at lines 517-518 asserts a global match.

**Adversarial verification.** The full unified diff mode renders more state, but `--summary` is a separately advertised mode and its output makes an absolute claim. Unknown keys need not be summarized; the omitted fields are typed, active configuration owned by this binary. A config changing only `storage.backup_interval_minutes` or `amq.inject.verify_envelope` is a direct reproduction. P1.

## P1-28 — Sole configuration writes lack an atomic replacement rail

**Impact.** Interruption, disk exhaustion, or a short/failed direct write can truncate the only configuration file; no previous version or temporary replacement remains.

**Baseline evidence.** `3d52074 — src/config.rs:929-964 — ensure_config` writes first-boot and deprecated/migrated configuration directly. `3d52074 — src/config.rs:1712-1859 — save_config` carefully preserves TOML structure in memory but commits through a final `fs::write(config_path, ...)`. `3d52074 — src/cli.rs:531-553 — run_regenerate` likewise directly overwrites the sole file. None writes a same-directory temporary, syncs, renames, or preserves a backup.

**Adversarial verification.** `fs::write` reports ordinary errors, so this is not a silent-error claim; the missing rail is preservation of the previous valid file after truncation/open succeeds but completion fails or the process/host stops. SQLite has deliberate backup/integrity machinery, while the equally essential config does not. Because failure likelihood depends on interruption rather than a deterministic normal input, the finding is P1 rather than P0. P0-01 separately covers deterministic installer clobber.

## Subsystem conclusion

The UI has a substantial test corpus and generally clear state separation, but several local functions contradict declared invariants. Fixes can stay narrow: persist-before-commit, unify one default source, share runtime key-match semantics with validation, move the listed operations through existing workers, enumerate typed fields in summary, and use one atomic write helper. A UI framework or configuration-layer rewrite would not be supported by this evidence.
