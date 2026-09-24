# Phase 07 — Config property tests and migration coverage

**Track:** B (safety net) · **Parallel-safe with:** 05, 06
**Depends on:** nothing (benefits from 05) · **Blocks:** 12 — **hard gate**

## Goal

Establish the properties that a 531-line `config_schema()` split would otherwise
silently regress, and close the migration-arm and save-path gaps that currently have
no coverage at all.

## Evidence

- **No exhaustive template-coverage test exists.** Today's coverage is
  `assert!(rendered.contains("…"))` spot-checks (`config.rs:3261, 3470, 4015, 4150,
  4510, 4559, 4807`) plus round-trips (`:3446, 3463, 3469, 3480, 3555, 3572`) and
  `tests/limits.rs:72, 99`. **Nothing asserts that every schema field reaches the
  template** — precisely the property a table split regresses, and no existing test
  would catch a dropped `ConfigEntry`.
- **`save_config` silently skips five sections.** No `patch_table_*` call exists for
  `[storage]`, `[auto_resume]`, `[limits]`, `[amq.inject]`, or `[amq.orchestrator]`.
  Latent today because nothing mutates them at runtime — but the moment a UI path
  writes one, the change appears to work and vanishes on restart, with no error.
- **`patch_projects` rebuilds `[[projects]]` wholesale** (`config.rs:2403-2442`),
  destroying hand-edited entries not present in `Config::projects`. This is the one
  place the otherwise-solid "preserve user edits" property does not hold.
- **The `migrate_config` 1→2 arm is untested** (`config.rs:92-102`, clearing the stale
  `"o"` binding). Every arm is covered only transitively by a 0→current round-trip;
  none by name with a realistic before/after. Arms 2→3 and 3→4 have non-trivial
  `uses_legacy_defaults` guards.
- **`MacroEntry` has no serde defaults** (`config.rs:215-218`). `surface` is required,
  so a hand-authored macro omitting it fails to deserialize and **hard-errors the
  entire config**, not just that macro. No test covers this.
- **No `deny_unknown_fields` anywhere** (zero occurrences in `config.rs`). A typo'd key
  is silently ignored; `[keys]` is the sole exception (`config.rs:3051-3053`).
- **`detect_conflicts_default_config_clean` (`keybindings.rs:2599-2607`) is the single
  most valuable guardrail** for the `keys/` split — it proves the shipped
  `BINDING_DEFS` never self-conflicts.
- **`dux config regenerate --yes` takes no backup** (`cli.rs:496-526`).

## In scope

Property tests over the schema, direct migration-arm tests, save-path coverage, and
the `BINDING_DEFS` invariants that Phase 13 depends on.

## Out of scope

- Splitting `config.rs` or `keybindings.rs` → Phases 12, 13.
- Adding the provider capability fields → Phase 11 (this phase's coverage protects
  that change).
- Fixing `reset --all`'s missing confirmation → Phase 10.

## Work items

1. **Exhaustive template-coverage property test — the headline item.** Reflect over
   `Config`'s fields and assert each is either emitted by `config_schema()` or listed
   in an explicit, commented `RENDER_ONLY_EXEMPT` set. Must fail if a `ConfigEntry` is
   dropped during the split. Pair with the inverse: every emitted key must map to a
   real field (no orphaned template rows).
2. **Assert the documentation tenet mechanically.** 53/53 keys currently carry a
   comment and `comment: None` appears zero times — lock that in with a test asserting
   no `ConfigEntry::Field` has `comment: None`, so the property is enforced rather
   than merely observed. `tests/limits.rs` already does this for one section; generalize.
3. **Test every `migrate_config` arm by name**, with a realistic before-config and an
   asserted after-config: 0→1, **1→2** (the untested one), 2→3 and 3→4 including both
   sides of their `uses_legacy_defaults` guards, and a full 0→current chain. Assert
   idempotency: running the chain twice equals running it once.
4. **Test `save_config` round-trips for all five unpatched sections.** Write a config
   with non-default `[storage]`, `[auto_resume]`, `[limits]`, `[amq.inject]`,
   `[amq.orchestrator]` values, load, mutate in memory, save, reload — and assert the
   mutation survived. **These tests are expected to fail on first write.** That is the
   point: they document the gap. Then add the missing `patch_table_*` implementations
   so they pass.
5. **Add the schema-reflection guard for `save_config`**: every field must be either
   patched by `save_config` or explicitly listed as render-only. This prevents section
   six from joining the five.
6. **Test `patch_projects`' destructive rebuild** (`config.rs:2403-2442`) — write a
   config with a hand-added `[[projects]]` entry absent from `Config::projects`, save,
   and assert what happens. Then decide: preserve unknown entries, or document the
   behaviour loudly in the config comment. **Recommendation:** preserve, since the
   file is meant to be hand-editable.
7. **Give `MacroEntry` serde defaults** (`config.rs:215-218`) so a macro missing
   `surface` degrades to a default instead of hard-erroring the whole config. Test the
   malformed-macro case explicitly.
8. **Add a load-time unknown-key warning.** Do not error — that would break the
   forward-compatibility guarantee that lets an old dux read a newer config. Collect
   unrecognised keys during deserialization and surface them once via the status line
   and the log, so a typo is visible rather than silent.
9. **Preserve and extend the keybinding guardrails** Phase 13 depends on:
   `detect_conflicts_default_config_clean` (`keybindings.rs:2599-2607`) and
   `lookup_declaration_order_wins` (`:2612-2638`). Add a test asserting the full
   `BINDING_DEFS` **action set** and **count** (80 variants, 13 palette-only), so a
   `defs/*.rs` mis-partition that drops or duplicates an entry fails loudly.
10. **Add same-key-different-scope regression coverage.** `ctrl-g` serves three actions
    in disjoint scopes (`keybindings.rs:816, 966, 1047`) — `ExitInteractive`,
    `GenerateCommitMessage`, `ExitCommitInput`. Assert all three resolve correctly, so a
    split that flattens scope handling fails.
11. **Make `dux config regenerate --yes` take a timestamped backup** and test it.
    Combined with item 6 and the `reset --all` gap (Phase 10), a user currently has two
    ways to lose hand-authored config with no recovery path.

## Acceptance criteria

- [ ] Template-coverage property test passes and **fails when a `ConfigEntry` is
      deliberately removed** — verify by doing it and reverting.
- [ ] A test asserts no `ConfigEntry::Field` has `comment: None`.
- [ ] Every `migrate_config` arm has a named test with realistic fixtures; the chain is
      proven idempotent.
- [ ] All five previously-unpatched sections round-trip through `save_config`.
- [ ] Schema-reflection guard prevents a sixth unpatched section.
- [ ] `patch_projects` behaviour is either fixed to preserve unknown entries or
      documented in the config file itself; either way it is tested.
- [ ] `MacroEntry` deserializes with `surface` omitted; malformed-macro test present.
- [ ] Unknown config keys produce a visible warning, not silence, and do not error.
- [ ] `BINDING_DEFS` action set and count pinned (80 total, 13 palette-only).
- [ ] `ctrl-g` three-scope resolution tested.
- [ ] `config regenerate --yes` writes a backup, and it is tested.
- [ ] `cargo test --all-features` green.

## Validation

```bash
cargo test --all-features config
cargo test --all-features keybindings
cargo test --test limits
cargo test --test storage_migrations

# prove the net catches the failure it exists for:
#   1. delete one ConfigEntry from config_schema()
#   2. cargo test -> the coverage property test MUST fail
#   3. git checkout
```

## Risks

| Risk | Mitigation |
|---|---|
| Reflection over `Config` needs a derive or manual list | A manually maintained field list is acceptable **only** if a test asserts it matches the struct; otherwise use a `syn`-based build script or a small proc-macro |
| Adding `patch_table_*` for five sections changes save behaviour | Nothing mutates them at runtime today, so the blast radius is nil — that is exactly why this is the safe moment to fix it |
| The unknown-key warning fires on legitimate forward-compat configs | Warn once, never error; word it as informational, and exclude keys under a documented forward-compat prefix if one is introduced |
| Item 4's tests fail on first write | Expected and intended — they are written to document the gap, then made green by the fix in the same phase |

## References

- `artifacts/research-config-keys-theme-cli.md` §5 Risk register (R1–R7, R10), §3.7
- `artifacts/research-app-monoliths.md` §Tooling available
