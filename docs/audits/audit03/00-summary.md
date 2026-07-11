# Audit 03 — executive summary

Audit date: 2026-07-11  
Audited baseline: `3d5207486cfd3e75b96fc3ea550430f0439da998` (`3d52074`)  
Scope: the complete 194-file baseline tree, including all source, wrappers, installers, tests, workflows, manifests, operations documentation, and prior audit evidence  
Result: **9 P0, 28 P1, 2 P2; supply-chain research COMPLETE**

## Path disambiguation

This is the completed audit report under `docs/audits/audit03/`. The similarly named `docs/plans/audits/audit03/` directory is historical planning and review evidence from the baseline. It is input evidence, not this report and not a substitute for it.

## Method and confidence

The audit followed the frozen `PLAN.md` audit phase (steps 2, 3, 4, and 6). Every baseline file was classified in [07-coverage-manifest.md](07-coverage-manifest.md) as read or skimmed; none was omitted. Static investigation covered correctness, security against the documented single-user/single-UID VM threat model, reliability/data loss, operability, architecture/maintainability, and supply chain. No build, test, Clippy, audit, deny, or Make target was run because the pre-change gate suite was being captured separately in this checkout.

The plan requested parallel read-only agents. Only one audit agent was available, so the accepted deviation was implemented as a systematic manifest pass followed by a separate adversarial verification pass. Each candidate below was re-opened at its cited baseline location and challenged for reachability, threat-model fit, existing guardrails, and counter-evidence in tests. Only candidates that survived that pass are findings. The detailed files record the attempted refutation.

Severity follows the frozen thresholds, not prior-audit labels:

- **P0:** exploitable within the documented threat model, actual data-loss/corruption path, or panic/hang on a normal supported input or lifecycle.
- **P1:** concrete reproduction or hot-path reasoning, a violated project tenet, or a missing safety rail with a credible failure path.
- **P2:** safe deletion, mechanical replacement, or test-proven simplification. Subjective cleanup is recorded as an observation, not a fixable finding.

## Ranked findings

| Rank | ID | Finding | Primary report |
|---:|---|---|---|
| P0 | P0-01 | Installer reruns overwrite the user's Dux configuration | [install/purge](04-install-purge-operations.md#p0-01-installer-reruns-overwrite-the-users-dux-configuration) |
| P0 | P0-02 | Stale-inflight recovery can overwrite a newer queued message | [runtime/storage](01-core-runtime-storage.md#p0-02-stale-inflight-recovery-can-overwrite-a-newer-queued-message) |
| P0 | P0-03 | Purge deletes the only session identity after an earlier cleanup failure | [install/purge](04-install-purge-operations.md#p0-03-purge-deletes-the-only-session-identity-after-an-earlier-cleanup-failure) |
| P0 | P0-04 | Purge does not enforce its promised deletion containment | [install/purge](04-install-purge-operations.md#p0-04-purge-does-not-enforce-its-promised-deletion-containment) |
| P0 | P0-05 | Updating `.git/info/exclude` converts a read failure into destructive overwrite | [runtime/storage](01-core-runtime-storage.md#p0-05-updating-gitinfoexclude-converts-a-read-failure-into-destructive-overwrite) |
| P0 | P0-06 | PTY teardown can block forever when a descendant keeps the slave open | [runtime/storage](01-core-runtime-storage.md#p0-06-pty-teardown-can-block-forever-when-a-descendant-keeps-the-slave-open) |
| P0 | P0-07 | A finite watch capture can panic reset processing | [runtime/storage](01-core-runtime-storage.md#p0-07-a-finite-watch-capture-can-panic-reset-processing) |
| P0 | P0-08 | Unicode macro text can panic the normal render path | [app/UI](02-app-ui-config.md#p0-08-unicode-macro-text-can-panic-the-normal-render-path) |
| P0 | P0-09 | The Codex wrapper disables hook trust review by default | [AMQ/wrappers](03-amq-transport-wrappers.md#p0-09-the-codex-wrapper-disables-hook-trust-review-by-default) |
| P1 | P1-01 | Custom `STATE_ROOT` installs are split across custom and hard-coded roots | [install/purge](04-install-purge-operations.md#p1-01-custom-state_root-installs-are-split-across-custom-and-hard-coded-roots) |
| P1 | P1-02 | Wrapper identity claiming has a check-then-create race | [AMQ/wrappers](03-amq-transport-wrappers.md#p1-02-wrapper-identity-claiming-has-a-check-then-create-race) |
| P1 | P1-03 | `_unrouted` is both a valid handle and a reserved routing sentinel | [AMQ/wrappers](03-amq-transport-wrappers.md#p1-03-_unrouted-is-both-a-valid-handle-and-a-reserved-routing-sentinel) |
| P1 | P1-04 | Strict HMAC mode does not provide a coherent authenticated-delivery boundary | [AMQ/wrappers](03-amq-transport-wrappers.md#p1-04-strict-hmac-mode-does-not-provide-a-coherent-authenticated-delivery-boundary) |
| P1 | P1-05 | Doctor's hash, JSON, anonymization, and read-only contracts are broken | [install/purge](04-install-purge-operations.md#p1-05-doctors-hash-json-anonymization-and-read-only-contracts-are-broken) |
| P1 | P1-06 | CI shell lint omits the highest-risk extensionless overlay scripts | [supply chain](05-ci-release-supply-chain.md#p1-06-ci-shell-lint-omits-the-highest-risk-extensionless-overlay-scripts) |
| P1 | P1-07 | The fork's release installer downloads a different upstream binary | [install/purge](04-install-purge-operations.md#p1-07-the-forks-release-installer-downloads-a-different-upstream-binary) |
| P1 | P1-08 | The root installer ignores the release checksum and attestation | [supply chain](05-ci-release-supply-chain.md#p1-08-the-root-installer-ignores-the-release-checksum-and-attestation) |
| P1 | P1-09 | Release packaging feeds an ISO timestamp to GNU tar's epoch syntax | [supply chain](05-ci-release-supply-chain.md#p1-09-release-packaging-feeds-an-iso-timestamp-to-gnu-tars-epoch-syntax) |
| P1 | P1-10 | Release and security-gate tools are selected from a moving latest version | [supply chain](05-ci-release-supply-chain.md#p1-10-release-and-security-gate-tools-are-selected-from-a-moving-latest-version) |
| P1 | P1-11 | Claude Peers is installed and updated from an unpinned default branch | [supply chain](05-ci-release-supply-chain.md#p1-11-claude-peers-is-installed-and-updated-from-an-unpinned-default-branch) |
| P1 | P1-12 | Provider wrappers enforce no minimum safe CLI version | [supply chain](05-ci-release-supply-chain.md#p1-12-provider-wrappers-enforce-no-minimum-safe-cli-version) |
| P1 | P1-13 | Two newly open RustSec advisories make the exact-lock security gate fail | [supply chain](05-ci-release-supply-chain.md#p1-13-two-newly-open-rustsec-advisories-make-the-exact-lock-security-gate-fail) |
| P1 | P1-14 | Configured periodic backups are never started | [runtime/storage](01-core-runtime-storage.md#p1-14-configured-periodic-backups-are-never-started) |
| P1 | P1-15 | Schema migrations are not atomic despite documentation saying they are | [runtime/storage](01-core-runtime-storage.md#p1-15-schema-migrations-are-not-atomic-despite-documentation-saying-they-are) |
| P1 | P1-16 | Session settings mutate live memory before persistence succeeds | [app/UI](02-app-ui-config.md#p1-16-session-settings-mutate-live-memory-before-persistence-succeeds) |
| P1 | P1-17 | AMQ scanning caps an arbitrary directory subset before sorting | [runtime/storage](01-core-runtime-storage.md#p1-17-amq-scanning-caps-an-arbitrary-directory-subset-before-sorting) |
| P1 | P1-18 | Busy and missing-PTY deliveries never enter the timeout-warning state | [runtime/storage](01-core-runtime-storage.md#p1-18-busy-and-missing-pty-deliveries-never-enter-the-timeout-warning-state) |
| P1 | P1-19 | The documented `u64::MAX` quiet-window escape hatch holds forever | [runtime/storage](01-core-runtime-storage.md#p1-19-the-documented-u64max-quiet-window-escape-hatch-holds-forever) |
| P1 | P1-20 | Generated and deserialized UI defaults disagree | [app/UI](02-app-ui-config.md#p1-20-generated-and-deserialized-ui-defaults-disagree) |
| P1 | P1-21 | Keybinding conflict validation disagrees with runtime matching | [app/UI](02-app-ui-config.md#p1-21-keybinding-conflict-validation-disagrees-with-runtime-matching) |
| P1 | P1-22 | Diff generation turns read errors into invented empty files | [runtime/storage](01-core-runtime-storage.md#p1-22-diff-generation-turns-read-errors-into-invented-empty-files) |
| P1 | P1-23 | The main UI path still performs blocking filesystem and Git work | [app/UI](02-app-ui-config.md#p1-23-the-main-ui-path-still-performs-blocking-filesystem-and-git-work) |
| P1 | P1-24 | Changed-file polling can fork once per untracked file every two seconds | [runtime/storage](01-core-runtime-storage.md#p1-24-changed-file-polling-can-fork-once-per-untracked-file-every-two-seconds) |
| P1 | P1-25 | Purge derives AMQ and log targets differently from the runtime | [install/purge](04-install-purge-operations.md#p1-25-purge-derives-amq-and-log-targets-differently-from-the-runtime) |
| P1 | P1-26 | Config diff omits behavior-changing sections and arguments | [app/UI](02-app-ui-config.md#p1-26-config-diff-omits-behavior-changing-sections-and-arguments) |
| P1 | P1-27 | Lifecycle persistence failures can orphan worktrees and report false success | [runtime/storage](01-core-runtime-storage.md#p1-27-lifecycle-persistence-failures-can-orphan-worktrees-and-report-false-success) |
| P1 | P1-28 | Sole configuration writes lack an atomic replacement rail | [app/UI](02-app-ui-config.md#p1-28-sole-configuration-writes-lack-an-atomic-replacement-rail) |
| P2 | P2-01 | Terminal widths use Unicode scalar counts despite a width-aware renderer | [architecture](06-architecture-modernity.md#p2-01-terminal-widths-use-unicode-scalar-counts-despite-a-width-aware-renderer) |
| P2 | P2-02 | A private fake re-export and no-op function are safely deletable | [architecture](06-architecture-modernity.md#p2-02-a-private-fake-re-export-and-no-op-function-are-safely-deletable) |

## Coverage statement

Baseline tree: **194 files**. **83 files read**, **111 files skimmed**, **0 files not covered**. “Read” means the complete file or every behaviorally relevant section was inspected; “skimmed” means structure, symbols, tests, generated/binary metadata, or historical evidence was systematically inspected and targeted sections were opened as needed. The complete one-row-per-file accounting is in [07-coverage-manifest.md](07-coverage-manifest.md).

## Supply-chain completeness

**COMPLETE as of 2026-07-11.** The exact 375-package `Cargo.lock` set was queried against OSV/RustSec with no date cutoff; all pinned GitHub Actions were queried through the GitHub Advisory Database and their current tag commits recorded; agent-CLI advisories/CVEs and hook-trust semantics were checked; and all non-Cargo components shipped, downloaded, or selected at runtime were inventoried. Mandatory feeds were reachable. The retained query results, component versions, source URLs, and query date are in [05-ci-release-supply-chain.md](05-ci-release-supply-chain.md).

## Pre-change health record

Captured independently by Claude at baseline `3d52074` on 2026-07-11, using the exact CI invocations:

| Gate | Result |
|---|---|
| `cargo fmt --check` | ✅ pass |
| `cargo clippy --all-targets --all-features -- -D warnings` | ✅ pass |
| `cargo test --all-features` | ✅ pass (913 lib tests + integration suites, 0 failures) |
| `cargo audit --deny warnings --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2024-0384` | ❌ **fail** — 2 denied warnings: RUSTSEC-2026-0190 (`anyhow 1.0.102`), RUSTSEC-2026-0205 (`scc 2.4.0`) — pre-existing; matches finding P1-13 |
| `cargo deny --all-features check` | ❌ **fail** — same two advisories — pre-existing; matches P1-13 |
| `make overlay-test` | ❌ **fail** — ShellCheck SC2329: `sha256_file()` never invoked in `dux-amq/scripts/dux-amq-doctor:87` — pre-existing; corroborates P1-05 |

These three failures exist **before** any audit fix and must not be attributed to remediation. The independently captured gate failures match the audit's static predictions (P1-13, P1-05) exactly.

## Expected behavior changes in the later fix phase

No runtime behavior changed during this audit phase. Remediation of confirmed findings is expected to change these externally visible behaviors:

- rerunning installers will preserve existing Dux configuration and use the configured state root consistently;
- Codex hooks will retain upstream trust review unless a user explicitly chooses the bypass;
- purge will reject out-of-root targets, retain recoverable identity after partial failure, and derive runtime-compatible AMQ/log paths;
- AMQ recovery will never overwrite a newer queued body, reserved routing will be unambiguous, strict authentication will bind and atomically deliver the verified body, and all deferred reasons will time out visibly;
- PTY shutdown will be bounded even when descendants retain descriptors;
- Unicode macro/list rendering and terminal column calculations will be width-safe;
- reset-time capture conversion will reject unrepresentable durations instead of panicking;
- migrations and configuration persistence will use explicit atomicity/durability rails, and configured backups will actually run;
- provider/install/release tooling will enforce provenance, version floors, checksums, and deterministic inputs;
- UI/storage failures will be surfaced before in-memory success is committed, and hot polling will avoid unbounded subprocess amplification.

## Prior-audit cross-reference

Prior reports were used as evidence, not as the framing taxonomy:

- Audit 01's P2-8 already noted that `dux config regenerate --yes` overwrote manual edits. The fresh baseline trace proves unconditional execution on installer rerun and therefore classifies it as P0-01 under this plan's data-loss threshold.
- Audit 02's P1-F identity-collision repair remains check-then-create (P1-02). Its P1-G reader-thread repair introduced or retained an unbounded join ordering (P0-06). Its P1-W backup/migration work leaves periodic backup wiring absent and repeats an incorrect transaction claim (P1-14/P1-15).
- Audit 02's P0-D main-thread-blocking class remains present, but the frozen audit03 threshold places the surviving, concretely traced cases at P1-23/P1-24 rather than inheriting the old label.
- Audit 02's purge and doctor deliverables now exist, but fresh traces show destructive failure ordering/containment gaps and broken doctor contracts (P0-03/P0-04/P1-05/P1-25).
- Supply-chain improvements from the earlier audits are real: pinned artifact hashes match and pinned Actions currently have no published Action-ecosystem advisories. They do not resolve floating provider/tool versions, the installer verification gap, or newly open exact-lock RustSec results (P1-08 through P1-13).

## Disposition

Every reportable item starts in state `confirmed`, as required for the audit phase. The fix phase must update status and proof in [99-disposition.md](99-disposition.md); it must not silently remove or renumber a finding.
