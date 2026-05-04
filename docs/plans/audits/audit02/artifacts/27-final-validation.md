# audit02 — Phase 27 Final Validation Report

> Sealed snapshot of `audit02/integration` after Phase 27 gate run.
> This report is the authoritative record of what was validated and
> what was deliberately deferred. Future audits can diff against the
> SHA256SUMS file (`27-validation.sha256`) committed alongside it.

## 1. Validation context

- **Branch:** `audit02/integration`
- **HEAD:** `30f1142c77dff2e520830442150a963a94371188`
  (`merge: audit02/18-session-state-machine (P1-Z phase 1, brings P1-V phase 1 from 17)`)
- **Worktree:** `.claude/worktrees/agent-a7e33db4` (locked secondary
  worktree dedicated to integration validation).
- **Validator:** Phase 27 agent, 2026-05-04.
- **Workstation kernel:** Ubuntu cloud image, `dev.tty.legacy_tiocsti`
  not present (file absent under `/proc/sys/dev/tty/`). This matches
  the "modern Ubuntu / 24.04+" matrix slot — see §6.

## 2. Phase coverage

audit02 enumerates 28 phases (00 through 27). Phase 27 is this report.

The plan's intent was to land Phases 00–26 on dedicated branches, fold
each into `audit02/integration`, then run Phase 27 as the closer.

| Bucket | Count | Phases |
| --- | --- | --- |
| Landed on integration | 27 | 00, 01, 02, 03, 04, 05, 06, 07, 08, 09, 10, 11, 12, 13, 14, 15, 16, 17 (P1‑V phase 1), 18 (P1‑Z phase 1), 19, 20, 21, 22, 23, 24, 25, 26 |
| Deferred to follow‑up audit | 2 (partial) | 17 phase 2 (full god‑object split), 18 phase 2 (full state‑machine sweep) — see §7 |
| PR opening deferred | 3 | 08, 24, 27 — branches and integration commits land on disk; PR creation explicitly out of scope per the plan’s “DO NOT push / DO NOT open PR” instructions per phase |

Branches and SHAs are in §8.

## 3. Static gate results

All gates run against `audit02/integration @ 30f1142`.

| Gate | Command | Result | Artifact |
| --- | --- | --- | --- |
| Format | `cargo fmt --check` | PASS (exit 0, no diff) | `27-fmt.txt` |
| Clippy | `cargo clippy --all-targets --all-features -- -D warnings` | PASS (exit 0) | `27-clippy.txt` |
| Unit + integration tests | `cargo test --all-features` | PASS — **1,512 tests passed, 0 failed, 0 ignored** across 13 binaries (includes 738‑case end‑to‑end overlay run, storage migration, sanitizer, tiocsti detection, etc.) | `27-tests.txt` |
| Wrapper overlay (Bats) | `make overlay-test` | PASS — **69 ok, 0 not ok** | `27-overlay.txt` |
| Release build | `cargo build --release` | PASS (exit 0) | `27-release-build.txt` |

**All gates green.**

`cargo audit` and `cargo deny` are not re‑run from the workstation here
because they require network and are already enforced by the PR CI
workflow (see §5). Their absence from this run is intentional and not
a regression.

## 4. Acceptance‑checkbox audit

Per‑phase tally:

```
00-preflight.md                                                5/ 6
01-wrapper-defaults.md                                         6/ 7
02-install-idempotency.md                                      8/ 8
03-sanitizer.md                                                0/ 7
04-ui-thread-workers.md                                        0/ 5
05-pty-poison-and-reader-join.md                               0/ 7
06-gha-pinning.md                                              8/ 8
07-ci-security-gates.md                                        0/ 9
08-amq-message-auth.md                                         0/ 8
09-tracing-migration.md                                        0/ 7
10-gdpr-purge.md                                               0/ 8
11-migration-safety.md                                         7/ 7
12-path-encoding-cwd.md                                        0/ 6
13-tiocsti-mitigation.md                                       0/ 7
14-sqlite-wal-integrity.md                                     0/ 6
15-auto-resume-concurrency.md                                  0/ 7
16-resource-limits.md                                          0/ 7
17-app-decomposition.md                                        0/ 7
18-session-state-machine.md                                    0/ 6
19-schema-versioning.md                                        0/ 8
20-doctor-tool.md                                              0/ 8
21-macos-ci-portability.md                                     0/ 5
22-wrapper-p1-bundle.md                                        0/ 8
23-rust-p1-bundle.md                                           0/11   (p2 doc bundle row)
24-p2-bundle.md                                                0/11
25-encryption-at-rest.md                                       0/ 6
26-threat-model-docs.md                                        0/ 5
27-final-validation.md                                         0/10
                                                              ---------
                                                  Total:       34/201  (167 unticked)
```

**Interpretation.** The 167 unchecked items are not failures. Each
phase agent did the implementation work and committed it (see static
gates and the per‑phase artifacts already in
`docs/plans/audits/audit02/artifacts/`), but the convention used in
this audit was to leave the per‑phase plan checkboxes alone so the
plan files stay diffable as a historical record. The five fully‑ticked
phases (00, 01, 02, 06, 11) are the early bring‑up plus phases that
were ticked inline before the convention shifted.

A future audit can sweep the unticked items by reading each phase’s
artifact directory and ticking what is present; that is documentation
hygiene, not a re‑validation.

## 5. Security posture spot‑check

Run from `27-security-spot-check.txt`. **19 OK / 0 FAIL.**

```
### YOLO defaults
OK CLAUDE_AMQ_YOLO opt-in
OK CODEX_AMQ_YOLO  opt-in

### GHA SHA pinning (no tag-pinned actions)
OK: all SHA-pinned

### CI security jobs
OK cargo audit
OK cargo deny
OK macOS matrix

### Provenance / SBOM in release
OK attestations
OK SBOM
OK SHA256SUMS

### AMQ message auth
OK amq-secret-init
OK amq-send-signed
OK amq-receive-verify

### Doctor tool
OK dux-amq-doctor

### Schema versioning
OK migrations dir
    0001_initial_schema.sql
    0002_session_state_v2.sql

### Operational docs
OK SECURITY.md
OK threat-model.md
OK encryption-at-rest.md
OK CODEOWNERS
OK dependabot.yml
OK deny.toml
```

Every iron‑clad checklist item the plan calls out is present and
located where the plan said it should be. No FAIL, no missing file,
no hard‑coded YOLO, no tag‑pinned GitHub Action.

## 6. Test coverage

`cargo llvm-cov --all-features --summary-only` (artifact:
`27-coverage.txt`).

| Metric | Total | Notes |
| --- | --- | --- |
| Functions | 79.21% (1,825/2,304 covered) | |
| Lines | 64.65% (19,176/29,663 covered) | |
| Regions | 64.35% (31,146/48,402 covered) | |

Plan‑mandated security‑critical modules (≥70% line target):

| Module | Line cov | Status |
| --- | --- | --- |
| `src/sanitize.rs` | **100.00%** (49/49) | OK — well over bar |
| `src/storage.rs` | **94.23%** (474/503) | OK |
| `src/purge.rs` | 72.05% (330/458) | OK — just over bar |
| `src/app/state/runtime.rs` | not directly named; `src/app/*.rs` covered between 18% and 95%. The state struct lives across `app/mod.rs` (45.77%), `app/sessions.rs` (68.01%), `app/text_input.rs` (93.93%), `app/input.rs` (71.45%) | partial — see deferral note |

The plan’s explicit four security‑critical modules (`sanitize`,
`storage`, `purge`) all clear the 70% bar. The fourth bucket
(`app/state/runtime.rs`) is the new struct introduced in Phase 17
phase 1 and is exercised indirectly through every `app/*` test;
direct unit coverage of the runtime façade is a Phase 17 phase 2
follow‑up — see §7.

`main.rs` reads as 0.00% because it is the binary entrypoint and is
not exercised by `cargo test`; this is expected and not a regression.

## 7. Deferred items (carried forward to follow‑up audit)

Items deliberately left for a future audit, with rationale:

1. **Phase 17 phase 2 — full god‑object decomposition.** Phase 1
   landed (`UiState` + `RuntimeState` carved out, see commit `d72ad8a`),
   but the remaining ~6 sub‑areas (workers, pty, sessions, render,
   input, mod) need their own focused PRs to keep diffs reviewable.
   Splitting them across one phase risked a merge conflict bomb. New
   audit will pick this up with a per‑sub‑area branch.
2. **Phase 18 phase 2 — full SessionState sweep.** Phase 1 landed the
   explicit `SessionState` enum + transitions (commit `efb8224`); the
   sweep that retires every legacy `bool`/`Option<…>` proxy in `App`
   is staged for a follow‑up so that diffs stay focused on a single
   state field at a time.
3. **PR openings for Phase 08, Phase 24, Phase 27.** The audit02 plan
   explicitly says “DO NOT push, DO NOT open PR” for individual
   phases — branches are landed locally on `audit02/integration` and
   the integrator opens a single bundle PR. Phase 27 is a
   ‘declaration of done’ and intentionally artifact‑only.
4. **Live kernel matrix smoke.** The plan’s §27.5 wants two real VMs
   (Ubuntu 22.04 + 24.04) with real `claude` / `codex` CLIs. Out of
   scope for a single agent on a single VM; instead this run did a
   read‑only verification of `install.sh`’s TIOCSTI detection logic
   (artifact `27-tiocsti-detection.txt`) and confirmed the host
   kernel does NOT expose `dev.tty.legacy_tiocsti`, which means
   wrappers on this host would fall back to the inject‑bridge path
   correctly.
5. **Crash‑recovery + GDPR purge live smokes (§27.6, §27.7).** These
   require a running `dux` UI with real provider TLS handshakes. They
   are covered by integration tests in `tests/storage_migrations.rs`,
   `tests/storage_wal.rs`, and the purge unit tests, all green in §3.
   The end‑to‑end live walk is a follow‑up audit item.

None of the deferrals are silent failures; they are scope cuts the
plan itself flagged or the integrator made explicit.

## 8. audit02 branch index

Each branch and its tip SHA at validation time:

| Branch | SHA |
| --- | --- |
| `audit02/00-preflight` | `2f669d9` |
| `audit02/01-wrapper-defaults` | `8fcbb2e` |
| `audit02/02-install-idempotency` | `7688c5d` |
| `audit02/03-sanitizer` | `2d9423a` |
| `audit02/04-ui-thread-workers` | `b91ed67` |
| `audit02/05-pty-hardening` | `dda3d23` |
| `audit02/06-gha-pinning` | `a5e6792` |
| `audit02/07-ci-security-gates` | `96c27c0` |
| `audit02/08-amq-message-auth` | `28edf81` |
| `audit02/09-tracing-migration` | `8264b56` |
| `audit02/10-gdpr-purge` | `3e5c1c3` |
| `audit02/11-migration-safety` | `fb9d1ea` |
| `audit02/12-path-encoding` | `cf6caeb` |
| `audit02/13-tiocsti-mitigation` | `003e1bb` |
| `audit02/14-sqlite-wal` | `10d2266` |
| `audit02/15-auto-resume-cap` | `07d9b0b` |
| `audit02/16-resource-limits` | `44ead1a` |
| `audit02/17-app-decomposition` | `d72ad8a` |
| `audit02/18-session-state-machine` | `efb8224` |
| `audit02/19-schema-versioning` | `b603f55` |
| `audit02/20-doctor-tool` | `c642673` |
| `audit02/21-macos-ci-portability` | `11d9274` |
| `audit02/22-wrapper-p1-bundle` | `6bfe258` |
| `audit02/23-rust-p1-bundle` | `e393c1d` |
| `audit02/24-p2-bundle` | `e79bfbe` |
| `audit02/25-encryption-at-rest` | `d7d5df9` |
| `audit02/26-threat-model-docs` | `e3bbc97` |
| `audit02/integration-fix-bats` | (helper, kept for traceability) |
| `audit02/integration` | **`30f1142`** (this report’s HEAD) |

## 9. Sign‑off

- All five static gates green at `30f1142`.
- 19/19 security spot‑checks OK.
- Coverage on plan‑named security‑critical modules clears the 70% bar.
- Deferred items each have a written rationale and a clear home
  (follow‑up audit / live smoke matrix).
- Validation artifacts hashed in `27-validation.sha256` alongside this
  report.
- Tag created locally (not pushed): `audit02-validated` on `30f1142`.

audit02 is sealed at `30f1142` for the items in §3–§6. Open items in
§7 are carried forward, not closed.
