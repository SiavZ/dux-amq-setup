# Phase 01 — Toolchain upgrade and dependency currency

**Track:** A (foundation) · **Parallel-safe with:** 02, 03, 04
**Depends on:** nothing · **Blocks:** 18 (new crate must be linted by the new toolchain), and weakly all of Track D

## Goal

Bring the compiler and the dependency graph current, clear both standing RustSec
ignores, and remove the yanked crate from the tree — so that every later phase is
written against the lint set CI will actually enforce.

## Evidence

- `rust-toolchain.toml:12` pins **1.88.0** (2025-06-23). Current stable is **1.98.0**
  (2026-08-18) — ten releases, ~14 months. Verified via `rustup check`.
- The pin is duplicated in every workflow: `pr.yml:26,49,80,107`, `test.yml:29,55`,
  `release.yml:79`. A bump is a 7-line diff.
- `cargo clippy --all-targets --all-features -- -D warnings` exits 0 today — but
  every lint added in 1.89–1.98 is invisible, so that green is weaker than it looks.
  Because clippy is `-D warnings`, deferring produces one large cleanup diff later.
- Dependency currency verified against `crates.io/api/v1/crates/<name>` on
  2026-08-31; 373 crates in `Cargo.lock`.

## In scope

Toolchain bump, dependency upgrades, removal of both `cargo audit --ignore` entries,
elimination of duplicate majors, and a documented position on `opaline`.

## Out of scope

- New CI gates → Phase 02.
- Release-pipeline changes → Phase 03.
- Any source refactor. If an upgrade forces API churn, fix it minimally here; the
  structural work belongs to Track D.

## Work items

1. **Bump `rust-toolchain.toml:12` to 1.98.0**, and the six hardcoded copies in
   `pr.yml:26,49,80,107`, `test.yml:29,55`, `release.yml:79`. Update the file's
   existing rationale comment — it currently argues 1.88 was the max MSRV across the
   resolved graph, which is a floor that expired.
2. **Fix every new clippy warning** the bump surfaces. Do not `#[allow]` past them
   without a written reason (see Phase 02's `#[allow]` scorecard rule).
3. **`cargo update` (lockfile only).** Clears the vulnerable `lru` pulled by
   `ratatui`, clears the **yanked `chacha20 0.10.0`**, and picks up ~12 patch/minor
   bumps. No API churn expected.
4. **`notify` 7 → 8.** This is the only way to drop **RUSTSEC-2024-0384** and one of
   the two standing `--ignore` entries at `pr.yml:128` / `test.yml:73`. Touches
   `src/watch/` — re-run `tests/watch_engine_integration.rs` (5 tests, real PTYs).
5. **`rand` 0.8 → 0.10** (two majors). `small_rng` still exists. Collapses the
   `0.8.6 + 0.10.1` duplicate that `petname` drags in.
6. **`petname` 3.0 → 3.2**, **`sysinfo` 0.35 → 0.39**, **`rusqlite` 0.39 → 0.40**
   (bundled SQLite moves with it — re-run `tests/storage_migrations.rs`),
   **`compact_str` 0.9 → 0.10**, **`serial_test` 3.5 → 4.0**, **`ratatui` 0.30.0 →
   0.30.2**, **`crokey` 1.4 → 1.5**, **`similar` 3.1 → 3.2**.
7. **Remove both `--ignore` flags** from `cargo audit` once 4 and 3 land. Delete the
   corresponding `RUSTSEC-2025-0141` / `RUSTSEC-2024-0384` exception rows from
   `deny.toml` and both workflows. **Note:** `dux-amq/tests/supply-chain-rails.bats`
   asserts those advisory IDs and the `2026-08-01` review date appear in all three
   files — that test must be updated in the same commit, not deleted.
8. **Resolve the duplicate `toml 0.8` / `toml_edit 0.22`.** They enter via
   `opaline`'s `toml ^0.8`. Either upstream a bump to `opaline` or accept and
   document. `deny.toml` sets `multiple-versions = "warn"`, so this is currently
   invisible in CI — consider promoting to `deny` with an explicit skip list.
9. **Replace or vendor `content_inspector` 0.2.4.** Last release and last repo push
   **2018-11-04** — 7.8 years, effectively abandoned. It is a BOM/binary sniffer;
   ~30 lines inline removes a frozen dependency from the SBOM. Coordinate with
   Phase 08 work item on `classify_untracked_file` (`git.rs:799`), which is its only
   caller and is being changed to read a 1 KiB prefix anyway.
10. **Document the `opaline` risk.** Published 2026-02-25 (six months old),
    **16,073 total downloads**, single maintainer, five releases in three months, MIT,
    compiled into the shipped binary. crates.io publishes are unsigned and neither
    `cargo vet` nor `cargo crev` is configured. It appears **nowhere in
    `SECURITY.md`**. Add a threat row (Phase 23 owns the table; this phase supplies
    the assessment) and bump 0.4.0 → 0.4.1.
11. **Add an MSRV job** to `pr.yml` so the declared minimum is actually tested rather
    than assumed.

## Acceptance criteria

- [ ] `rust-toolchain.toml` and all six workflow copies read 1.98.0.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0 under 1.98.0.
- [ ] `cargo audit` passes with **zero** `--ignore` flags.
- [ ] `cargo deny --all-features check` passes; no yanked crates in `Cargo.lock`.
- [ ] `rand`, `notify`, `sysinfo`, `rusqlite`, `compact_str`, `serial_test`,
      `ratatui`, `crokey`, `similar`, `petname` at the versions in work items 4–6.
- [ ] No duplicate `rand` majors in `Cargo.lock`.
- [ ] `content_inspector` removed from `Cargo.toml`, or a written justification for
      keeping an abandoned dependency recorded in `deny.toml`.
- [ ] `supply-chain-rails.bats` updated to match the new advisory state and passing.
- [ ] `cargo test --all-features` green: 1,139 distinct tests, no new failures.
- [ ] MSRV job present and green.
- [ ] `opaline` risk assessment written and handed to Phase 23.

## Validation

```bash
rustc --version                                   # expect 1.98.0
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo audit                                       # no --ignore
cargo deny --all-features check
cargo tree -d                                     # inspect remaining duplicates
bats dux-amq/tests/supply-chain-rails.bats
```

## Risks

| Risk | Mitigation |
|---|---|
| `notify` 7→8 changes watch semantics | `tests/watch_engine_integration.rs` exercises real PTYs end-to-end; run it before and after and diff behaviour |
| `rusqlite` 0.40 bundles a different SQLite | `tests/storage_migrations.rs` (662 lines) covers the migration chain; the doctor's read-only-DB test pins byte-for-byte non-mutation |
| Ten releases of new clippy lints is a large cleanup | It is smaller now than after 140 files are split — that is precisely why this phase is first |
| `rand` 0.10 API churn | `small_rng` survives; the surface used is small. Grep before upgrading |
| Removing `content_inspector` changes binary detection | Phase 08 rewrites the only call site; land them together or sequence 01 → 08 |

## References

- `artifacts/research-debt-ci-security.md` §Dependency currency, §CI & supply chain
- `artifacts/00-baseline.md` §Toolchain drift
