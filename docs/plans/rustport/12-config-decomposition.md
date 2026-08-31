# Phase 12 — `src/config.rs` decomposition

**Track:** D (decomposition) · **Parallel-safe with:** 13, 14, 15, 16, 17
**Depends on:** 02 (the 500-line gate), 05 (test migration), **07 (hard gate)** · **Blocks:** 24

## Goal

Split 3,237 production lines (4,952 with tests) into a `src/config/` tree where no file
exceeds 500 lines, without regressing a single config key or migration arm.

## Why 07 is a hard gate

`config_schema()` is a 531-line table. **Nothing today asserts that every schema field
reaches the template** — coverage is `assert!(rendered.contains(…))` spot-checks. A
dropped `ConfigEntry` during the split would be invisible. Phase 07's exhaustive
coverage property test is the only thing that makes this split safe. Do not start
without it green.

## Module tree

Target: `src/config/`, production lines shown; each module's migrated tests move with it.

```text
src/config/
  mod.rs                    ~90   Config struct, re-exports
  migrate.rs               ~100   migrate_config ladder, CONFIG_SCHEMA_CURRENT,
                                  default_schema_version (config.rs:60-154)
  deprecations.rs           ~95   DeprecatedConfigKey*, DEPRECATED_CONFIG_KEYS,
                                  apply_config_deprecations*, migrate_prompt_for_name (:1255-1346)
  schema/
    providers.rs           ~180   ProvidersConfig, ProviderCommandConfig, OneshotOutput
                                  (:241-295, 873-946, 1046-1076) — plus Phase 11's capability fields
    projects.rs            ~130   ProjectConfig, WorkspaceMode, WorkspaceConfig,
                                  deserialize_workspace_mode_override, new_project_id (:297-360)
    ui.rs                  ~130   UiConfig, EditorConfig, TerminalConfig, LoggingConfig (:248-266, 786-799, 947-991)
    keys_macros.rs         ~170   KeysConfig, MacroSurface, MacroEntry, MacrosConfig (:158-229, 839-859)
    limits.rs              ~200   StorageConfig, AutoResumeConfig, LimitsConfig + 11 default fns (:362-533)
    amq.rs                 ~250   AmqConfig, AmqInjectConfig (14 keys), AmqOrchestratorConfig + 17 default fns (:534-785)
  paths.rs                 ~180   DuxPaths, resolve_root, discover_root, expand_path, is_valid_var_name
                                  (:1077-1111, 3108-3237)
  load.rs                  ~180   ensure_config, load_config_read_only, registered_project_paths,
                                  shared-workspace validators (:1112-1254)
  template/
    entries.rs             ~320   FieldValue, CommentSource, ConfigEntry, first half of config_schema
    entries_amq.rs         ~220   config_schema tail (amq / keys / macros / projects)
    render.rs              ~280   render_config*, render_keys_config, render_macros_config,
                                  render_projects, render_terminal_config, TOML escapers (:1924-2017, 2476-2676)
    providers.rs           ~300   default_provider_commands, render_provider_config* (:2677-3045)
  save.rs                  ~460   save_config, write_config_atomic, atomic_write_with,
                                  ensure_table, all patch_* (:2019-2475)
  validate.rs               ~70   validate_keys, check_provider_available (:3046-3107)
```

`template/providers.rs` is the only module near the cap. **Phase 11 item 6 already
brings it from ~380 to ~300** by extracting the ~75-line Claude-only watch-rule example
block (`config.rs:2964-3035`) to an `include_str!` fragment — which simultaneously
retires tenet violation P12. Sequence 11 before this phase, or absorb that extraction here.

## Work items

1. **Confirm Phase 07's gates are green** — template coverage, per-arm migration tests,
   all five `save_config` sections round-tripping, `comment: None` count is zero.
2. **Create the tree above, one module per commit**, moving code without editing it.
   Start with the leaves (`schema/*`, `paths.rs`, `validate.rs`) and finish with
   `mod.rs`; the template and save modules are the risky ones and go last.
3. **Split `config_schema()` across `template/entries.rs` and `entries_amq.rs`** as two
   functions returning `Vec<ConfigEntry>`, concatenated in the caller. Run the coverage
   property test after **every** move, not once at the end.
4. **Keep `save.rs` under 500** — it is budgeted at ~460 and grows if Phase 07 adds the
   five missing `patch_table_*` implementations. If it exceeds the cap, split the
   `patch_*` family into `save/patch.rs` with `save/mod.rs` holding the orchestration
   and atomic-write path.
5. **Add file headers** per `CONVENTIONS.md` §1, with generated `# Uses` / `# Used by`
   trees. Regenerate with the Phase 02 tool as part of each commit so `--check` stays clean.
6. **Preserve `patch_projects`' semantics exactly** (`config.rs:2403-2442`) — Phase 07
   decides whether the wholesale rebuild is fixed; this phase must not change it
   incidentally.
7. **Do not touch the `migrate_config` arms' logic** (`config.rs:104-149`). They are
   frozen by the append-only schema policy. Moving them to `migrate.rs` is fine; editing
   them is not.

## Acceptance criteria

- [ ] `src/config.rs` no longer exists; `src/config/` matches the tree above.
- [ ] No file in `src/config/` exceeds 500 lines (`ci/check-file-length.sh` clean for
      this subtree).
- [ ] Every file has a `//!` header with `` ```text ``-fenced `# Uses` / `# Used by` trees;
      `module-trees --check` passes.
- [ ] Template coverage property test green after every commit, not just the last.
- [ ] All `migrate_config` arm tests green; arm logic byte-identical (`git diff` on the
      moved hunks shows pure relocation).
- [ ] `dux config diff` and `dux config regenerate` produce byte-identical output to
      pre-split — capture a golden file before starting and compare.
- [ ] `cargo test --all-features` green with the same test count.
- [ ] `cargo modules orphans --deny` and `dependencies --acyclic` pass.

## Validation

```bash
# capture BEFORE starting
cargo run -- config regenerate --yes --stdout > /tmp/config-before.toml

# after the split
cargo run -- config regenerate --yes --stdout > /tmp/config-after.toml
diff /tmp/config-before.toml /tmp/config-after.toml    # must be empty

cargo test --all-features
ci/check-file-length.sh
cargo modules dependencies --acyclic --bin dux
cargo run -p xtask -- module-trees --check
```

## Risks

| Risk | Mitigation |
|---|---|
| A `ConfigEntry` is dropped silently | Phase 07's property test; run it per commit. This is the entire reason 07 gates 12 |
| Rendered config drifts by a byte (whitespace, ordering) | The golden-file diff above is the acceptance criterion, not a spot-check |
| `save.rs` exceeds the cap once Phase 07's patches land | Pre-planned `save/patch.rs` split (work item 4) |
| Circular imports between `schema/*` and `template/*` | `cargo modules dependencies --acyclic` in CI catches it immediately |

## References

- `artifacts/research-config-keys-theme-cli.md` §2.1, §1.1, §5
