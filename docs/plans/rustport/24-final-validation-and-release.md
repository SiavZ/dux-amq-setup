# Phase 24 — Final validation and production release

**Track:** E (serialized) · **Runs alone, last**
**Depends on:** all of 01–23 · **Blocks:** nothing — this is the definition of done

## Goal

Prove the whole stack, flip every gate from warn to blocking, close the remaining
production-readiness gaps, and cut the release that the fork has never actually shipped.

## In scope

Cross-cutting verification, the gate flip, packaging and distribution, and the release.

## Out of scope

Nothing. If something is missing at this point, it is a defect in an earlier phase and
goes back there — **not into a follow-up bucket.** That is the rule this plan set was
written under.

## Work items

### A. Flip the gates from warn to blocking

1. **`ci/check-file-length.sh` becomes blocking.** 22 files violated it at baseline;
   Track D should have brought that to zero. **Any remaining violation must carry a
   written `// allow-long-file: <reason>` and be listed here** — an unexplained survivor
   means a Track D phase is incomplete.
2. **`module-trees --check` becomes blocking.** Every `.rs` file in both crates carries a
   `//!` header with `` ```text ``-fenced `# Uses` / `# Used by` trees, and the generated
   trees match the committed ones.
3. **`clippy::missing_docs_in_private_items` becomes blocking** across both crates.
4. **`cargo modules orphans --deny` and `dependencies --acyclic` blocking** for both crates.
5. **Branch protection enforces every context** listed in Phase 02, with
   `enforce_admins: true`, verified by reading the live API — not the document.

### B. Cross-cutting verification

6. **Full suite, both crates, both platforms.** `cargo test --workspace --all-features` on
   ubuntu-24.04 and macos-14, plus the complete bats suite against the Rust binary.
7. **Verify the test count went up, not sideways.** Baseline was 1,139 distinct Rust tests
   plus 123 bats. Track B added render snapshots, contract tests, and config property
   tests; Phases 19–20 added coverage for behaviours the bash never tested (identity
   priorities #2–#4, the oneshot `exec` short-circuits, the four unasserted doctor
   sections, the `bashrc` happy path, the bridge's invalid-JSON branch). **Record the
   final numbers.**
8. **Confirm `tests/git_portability.rs` actually runs.** It reported `running 0 tests` on
   macOS, and both tests `return` early rather than skip-marking when git is absent — so a
   git-less CI leg passes green vacuously. Fix the skip semantics and confirm real
   execution on both platforms.
9. **Confirm the two-phase inject machine now has coverage.** It had **zero** executable
   coverage at baseline — `tick_amq_inject` and `drain_inject_queue_dir` were invoked by no
   test, so `deliver_inject_body`/`deliver_inject_enter` were unreachable. It is also where
   the recent bug-fix commits landed. Phase 15's split and Phase 19's port must have closed
   this; verify explicitly.
10. **Confirm the PTY grid leak is now visible to CI.** The baseline Drop test
    (`pty.rs:2075-2103`) asserted only that `recv_timeout(2s)` succeeded and was designed to
    tolerate the detach branch — it passed whether or not the grid leaked. Phase 08's test
    must fail on a reintroduced leak; **verify by reintroducing it and reverting.**
11. **Run the bats suite as a non-root user.** `wrappers-p1.bats`'s P1-D seed tests
    **skip under root** (`chmod 000` is a no-op for root), so a containerized CI running as
    root silently skips the only test proving rsync-warning surfacing works.
12. **Run the environment-gated tests for real.** `install-idempotency.bats` tests 2–3 skip
    unless `/data` exists and both `dux` and `amq` are on `PATH`. Assert they **ran**, not
    merely that they did not fail.
13. **Memory acceptance, measured.** 8 panes at 200 cols with saturated scrollback under
    100 MiB RSS (baseline: ~400 MiB at those settings, ~816 MiB at 16 panes). Record the
    before/after numbers.
14. **Idle CPU acceptance, measured.** With the dirty-flag redraw and the wall-clock timers,
    record idle CPU with 10 sessions and compare to the baseline (~400k `BTreeMap` probes/s
    and ~2k `String` allocations/s on the render thread).
15. **Tenet sweep.** Mechanically re-verify each: no file over 500 lines; no raw `Color::*`
    in production render code; no hardcoded keybinding labels; all git invocations through
    `run_git`; no provider-name branches outside the recorded `peer.rs` decision; every
    config key documented; no byte-based string truncation.

### C. Remaining production-readiness gaps

16. **Shell completions** via `clap_complete` 4.6.9 for both binaries — bash, zsh, fish.
17. **Man pages** via `clap_mangen` 0.3.3.
18. **Packaging manifests.** There is no Homebrew tap for this fork and no `cargo publish`.
    Decide and execute: a tap formula, and/or publishing to crates.io. **A decision to not
    publish is acceptable; leaving it unstated is not.**
19. **Verify `--version` works** on both binaries (added in Phase 23) — with `strip = true`,
    this is the only way to identify a field binary.
20. **Upgrade path documentation.** Write `docs/operations/upgrading.md`: what a user with
    the bash overlay installed must do, what the installer migrates automatically, and what
    state on disk is preserved. Phase 20 made the installer do it; this documents it.
21. **Telemetry statement.** The research reached a definitive verdict on what the binary
    does and does not phone home. State it plainly in `README.md` and `SECURITY.md` —
    users of an agent orchestrator will ask.

### D. Release

22. **Cut the release**, following Phase 03's now-working pipeline: four targets, pinned OS
    versions, `cargo auditable build`, `strip`, CycloneDX SBOM, double-package + `cmp`
    reproducibility, build-provenance attestation, `SHA256SUMS`.
23. **Verify the artifact contains the fork's own code** — extract and confirm `peer.rs`,
    `amq_inject.rs`, `purge.rs`, `watch/`, and the `dux-amq-rust` applets are present. This
    is the check whose absence produced finding F1.
24. **Run the release smoke test** on clean Linux and clean macOS hosts: install via the
    documented command, then `dux --version`, `dux doctor`, create an agent, send a peer
    message, and resume a session.
25. **Confirm `/releases/latest` resolves** and the documented install path in `README.md`
    works end-to-end — the thing that returned HTTP 404 at baseline.

## Acceptance criteria

- [ ] All gates blocking; branch protection verified against the live API.
- [ ] Zero files over 500 lines, or each survivor has a written reason listed in the PR.
- [ ] Every `.rs` file in both crates has a header with accurate, generated module trees.
- [ ] Full suite green on ubuntu-24.04 and macos-14; full bats suite green against the
      Rust binary, run as non-root.
- [ ] Final test counts recorded and higher than the 1,139 + 123 baseline.
- [ ] `git_portability` runs real tests on both platforms.
- [ ] The inject state machine and the PTY grid leak both have tests that **fail when the
      defect is reintroduced** — verified by doing it and reverting.
- [ ] Environment-gated bats tests asserted to have run, not skipped.
- [ ] Memory: 8 panes @200 cols under 100 MiB RSS, measured and recorded.
- [ ] Idle CPU measured and recorded against baseline.
- [ ] Tenet sweep clean.
- [ ] Completions and man pages ship; `--version` works on both binaries.
- [ ] Packaging decision made and executed or explicitly recorded.
- [ ] `docs/operations/upgrading.md` and the telemetry statement written.
- [ ] Release cut; artifact **verified to contain fork code**; SBOM, attestation, and
      reproducibility checks pass.
- [ ] `/releases/latest` returns 200; the documented install works on clean Linux **and**
      clean macOS.
- [ ] `ARCHITECTURE.md` re-read and still accurate after 23 phases of change.

## Validation

```bash
# gates
ci/check-file-length.sh
cargo run -p xtask -- module-trees --check
cargo modules orphans --deny --bin dux
cargo modules dependencies --acyclic --bin dux
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check

# suites
cargo test --workspace --all-features
bats dux-amq/tests                    # as a non-root user, Rust binary on PATH

# supply chain
cargo audit                            # zero --ignore
cargo deny --all-features check

# release verification
gh release view <tag> --json assets
tar tzf dux-linux-amd64.tar.gz | grep -E 'peer|amq_inject|purge|watch'
curl -s -o /dev/null -w '%{http_code}\n' \
  https://api.github.com/repos/SiavZ/dux-amq-setup/releases/latest   # expect 200

# smoke, on clean Linux and clean macOS
curl -fsSL https://raw.githubusercontent.com/SiavZ/dux-amq-setup/main/install.sh | bash
dux --version && dux doctor && dux-amq --version
```

## Risks

| Risk | Mitigation |
|---|---|
| Flipping gates to blocking reveals stragglers late | Track D phases each assert their own subtree is clean; this phase should find nothing. If it does, the owning phase reopens |
| "Nothing deferred" pressure produces a rubber-stamp | Every criterion here is mechanically checkable or explicitly measured. Ticking one without running its command is the only way to fail this phase quietly |
| The release exposes code never shipped before | It is the first real release of this code — expect findings. Run the smoke test on genuinely clean hosts, not a developer machine |
| Measured memory/CPU targets are missed | Report the real numbers rather than adjusting the target. If 100 MiB is not reached, Phase 08 reopens with the measurement as evidence |

## References

- All 23 preceding phases
- `artifacts/research-debt-ci-security.md` §Production-readiness gaps, §Coverage map
- `artifacts/research-runtime-memory.md` §6 (coverage gaps G1–G8)
- `artifacts/bats-test-parity-matrix.md` §3 (untested behaviours), §4 (hazards 9, 10)
