# Phase 14 — `src/cli.rs` and `src/theme.rs` decomposition

**Track:** D (decomposition) · **Parallel-safe with:** 12, 13, 15, 16, 17
**Depends on:** 02, 05 · **Blocks:** 20 (the doctor module moves here before Phase 20 replaces its backend), 24

## Goal

Split the CLI surface (1,206 production lines) and the theme bridge (666 production
lines) into modules under the cap. These are the two lowest-risk splits in Track D and
make good first targets for validating the tooling.

## Priority note

**`theme.rs` is the lowest-priority split in the entire plan.** At 666 production lines
it is only 33% over the cap, is already coherent, and carries the strongest test in its
domain — `dux_dark_matches_original_palette` (`theme.rs:766-862`), an exhaustive
78-field oracle. Do `cli/` first; do `theme/` only when the tooling is proven.

## Module tree — `src/cli/`

```text
src/cli/
  mod.rs                   ~180   run, run_session, reject_unknown_flags*,
                                  print_config_help, print_session_help (:24-152, 337-359)
  config_cmd.rs            ~200   run_diff, run_diff_raw, run_diff_summary,
                                  config_summary_changes, diff_typed_value, summary_value,
                                  run_regenerate, truncate_display, render_config_for_diff,
                                  print_unified_diff (:394-558)
  reset.rs                 ~350   run_reset, reset_agent_data*, remove_session_worktree,
                                  resolve_reset_log_path, remove_*/prune_* helpers,
                                  WithContextPath (:360-393, 559-871)
  purge_cmd.rs             ~190   run_session_purge, run_session_purge_all,
                                  runtime_purge_config, format_plan (:153-336)
  doctor.rs                ~340   resolve_doctor_script, run_doctor, merge_doctor_json,
                                  emit_rust_section_text, render_rust_section_text,
                                  append_orphaned_sessions_text, build_rust_section_json,
                                  doctor_db_path, snapshot types, collect_sessions_snapshot (:872-1206)
```

## Module tree — `src/theme/`

```text
src/theme/
  mod.rs                   ~150   Theme struct (78 fields), SPINNER_FRAMES,
                                  DEFAULT_THEME_NAME, DUX_DARK_TOML, GITHUB_PR_* (:14-127)
  loader.rs                ~260   load, load_or_fallback, load_from_file, load_from_str,
                                  discover_available, ThemeSource, ThemeListing (:139-288)
  defaults.rs              ~135   register_dux_defaults (:290-418)
  convert.rs               ~120   into_ratatui and the opaline -> ratatui mapping (:429-442)
  style.rs                 ~120   the ~12 impl Theme style/badge helpers (:444-665)
```

## Work items

1. **Split `cli/` first**, one module per commit, leaves before `mod.rs`.
2. **`reset.rs` carries the Phase 10 D5 fix** (the missing confirmation on
   `reset --all`). If Phase 10 has landed, move the fixed code; if not, move it as-is —
   **do not fix it during the move.** Behaviour changes and relocations stay in separate
   commits.
3. **`doctor.rs` is the seam Phase 20 will re-point.** Today `resolve_doctor_script`
   (`cli.rs:879`) shells out to the `dux-amq-doctor` **bash** script, with a hardcoded
   binary name and a hardcoded relative fallback path (`cli.rs:892`,
   `"../dux-amq/scripts/dux-amq-doctor"`). Keep that interface intact here; Phase 20
   swaps the backend to the Rust applet. **Make the script path configurable** while
   you are in the file — it is listed as configurability gaps H24/H25.
4. **`theme/` second, and only after `cli/` proves the tooling.** The 78-field oracle
   test must stay green through every commit.
5. **Preserve `into_ratatui`'s mapping exactly.** The theme tenet was verified clean
   (zero real raw `Color::*` violations in production render code); a mistranslation
   here would be invisible except through Phase 06's snapshots — which is precisely why
   those snapshots exist. Run them.
6. **Add file headers** with generated trees per `CONVENTIONS.md` §1.
7. **Add `tests/` coverage for CLI parsing.** No file under `tests/` currently covers
   CLI parsing, themes, or keybindings — all three live only in inline modules. Promote
   at least the subcommand-dispatch and unknown-flag-rejection tests to `tests/cli.rs`,
   since those are genuinely black-box.

## Acceptance criteria

- [ ] `src/cli.rs` and `src/theme.rs` no longer exist; the trees above match.
- [ ] No file exceeds 500 lines.
- [ ] `dux_dark_matches_original_palette` green (78-field oracle).
- [ ] Every `dux` subcommand behaves identically — verify each one's `--help` output
      against a golden file captured before the split.
- [ ] Phase 06 theme/pane snapshots show **zero** diff.
- [ ] Doctor script path is configurable rather than hardcoded (H24, H25).
- [ ] `tests/cli.rs` exists and covers subcommand dispatch plus unknown-flag rejection.
- [ ] File headers present; `module-trees --check` passes.
- [ ] `cargo test --all-features` green, same test count.

## Validation

```bash
# capture BEFORE
for c in "" config session doctor reset purge; do
  cargo run -- $c --help > /tmp/help-before-$c.txt 2>&1 || true
done

# after
for c in "" config session doctor reset purge; do
  cargo run -- $c --help > /tmp/help-after-$c.txt 2>&1 || true
  diff /tmp/help-before-$c.txt /tmp/help-after-$c.txt
done

cargo test --all-features
cargo insta test          # theme snapshots must not move
ci/check-file-length.sh
bats dux-amq/tests/doctor.bats
```

## Risks

| Risk | Mitigation |
|---|---|
| `reset.rs` is the most destructive code in the tool | Move it mechanically; no logic edits. Phase 10 owns the behaviour change |
| Doctor's text output is pinned by bats (`%-30s`/`%-5s` widths, 9 section headers) | `doctor.bats` runs in CI; run it explicitly after moving `doctor.rs` |
| Theme mapping drift is invisible without snapshots | Phase 06 is not a formal dependency of this phase, but running its snapshots here is an acceptance criterion — do not skip it |
| CLI help text drifts | The golden-file diff above is mandatory, per subcommand |

## References

- `artifacts/research-config-keys-theme-cli.md` §2.3, §2.4, §1.3, §1.4, §3.5 (H24, H25)
- `artifacts/bats-test-parity-matrix.md` §doctor.bats
