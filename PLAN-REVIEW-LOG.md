# Plan Review Log: audit03 — full fresh sweep + autonomous remediation
Act 1 (grill) complete — plan locked with the user. MAX_ROUNDS=5.

## Round 1 — Codex
(model gpt-5.6-sol, reasoning max, codex-cli 0.144.1, thread 019f4d8a-abcd-7de2-9365-5f00a62b2970)

## Material findings

1. **Audit identity collision:** “audit03” already exists and is marked implemented/landed, so a new “fresh audit03” makes phase and security references ambiguous. **Fix:** rename this sweep `audit04`, or explicitly reconcile it as a continuation of [Audit03 Phase 01](/Users/siavash/Projects/dux-amq-setup/docs/plans/audits/audit03/01-session-settings-modal.md:17).

2. **Unsafe baseline mutation:** [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:10) commits 13 tracked changes plus untracked artifacts directly on `main`, contrary to the documented PR-only workflow. **Fix:** create an `audit04/baseline` branch, stage an explicit allow-list after reviewing for secrets/generated files, and leave `main` untouched.

3. **No pre-change health record:** the WIP is committed before any gates run, so existing failures can later be misattributed to audit fixes. **Fix:** capture the complete baseline gate results and commit SHA before auditing, preserving every pre-existing failure.

4. **“Full coverage” is false:** the fan-out omits substantial surfaces such as `src/purge.rs`, `src/provider.rs`, `src/watch/`, root installers/manifests, integration tests, and threat-model docs, while [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:37) explicitly permits skimming. **Fix:** assign every in-scope file through a coverage manifest or relabel the engagement as sampled.

5. **Parallel review is not reproducible:** agents are neither required to be read-only nor pinned to one immutable revision, making findings and line citations race against repository changes. **Fix:** give every reviewer a read-only checkout of the same baseline SHA and cite SHA + path + symbol/snippet.

6. **Severity and stopping criteria are undefined:** autonomous remediation of every subjective P2 “modernity” finding creates unbounded refactoring and regression risk. **Fix:** define P0/P1/P2 thresholds and require a reproducible defect, measured bottleneck, unsupported construct, or safe deletion before a finding becomes fixable.

7. **Security policy contradicts the repository threat model:** “no accepted-risk carve-outs” conflicts with the load-bearing single-UID assumptions and explicitly accepted risks in [SECURITY.md](/Users/siavash/Projects/dux-amq-setup/SECURITY.md:30), potentially producing disruptive security theater such as same-UID HMAC enforcement. **Fix:** evaluate controls against named assets, adversaries, and trust boundaries; treat any threat-model change as an explicit reviewed decision.

8. **P0 handling violates disclosure discipline:** completing and potentially publishing the whole report before fixing confirmed P0s delays remediation and conflicts with [SECURITY.md](/Users/siavash/Projects/dux-amq-setup/SECURITY.md:8). **Fix:** keep exploitable findings embargoed and fast-track confirmed P0 patches before public report details.

9. **Branch ancestry is unspecified:** it is unclear whether `p1`/`p2` are independent or stacked, what users merge, and where the report lives. **Fix:** declare `baseline ← p0 ← p1 ← p2`, preserve each review boundary, and run a final combined gate on the `p2` tip.

10. **Verification is weaker than existing CI:** the plan omits `cargo audit`, `cargo deny`, ShellCheck, `cargo test --all-features`, and the Linux/macOS matrix already required by [PR CI](/Users/siavash/Projects/dux-amq-setup/.github/workflows/pr.yml:29) and [overlay CI](/Users/siavash/Projects/dux-amq-setup/.github/workflows/overlay-ci.yml:32). **Fix:** require the exact repository CI gates on every affected tier and the final stacked tip.

11. **Supply-chain research is incomplete and non-reproducible:** the January 2026 cutoff misses older unresolved advisories, `Cargo.lock` excludes bundled native code and downloaded binaries, and unavailable feeds merely become a footnote. **Fix:** inventory every shipped component/version, capture official advisory/tool outputs without an arbitrary date cutoff, and mark the audit incomplete while mandatory feeds are unavailable.

12. **The frozen report can preserve false claims:** implementation often disproves or merges findings, but [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:29) forbids conclusions from evolving. **Fix:** freeze raw baseline evidence while maintaining a final disposition ledger of confirmed, refuted, fixed, and deferred findings with fix SHAs.

13. **Runtime and performance claims lack proof:** static review plus unit tests cannot validate PTY behavior, installer upgrades, crash recovery, AMQ delivery, or performance, several of which were already deferred by audit02. **Fix:** require targeted Linux/macOS live smokes, `dux-amq doctor` before/after evidence, and a benchmark/profile before accepting performance fixes.

VERDICT: REVISE
### Claude's response (round 1)

**Accepted in full** — findings 3, 4, 5, 6, 9, 10, 12:
- (3) Pre-change health record: full gate suite runs at the baseline SHA before auditing; pre-existing failures recorded.
- (4) Coverage manifest: every in-scope file (incl. purge.rs, provider.rs, watch/, root installers, integration tests, threat-model docs) assigned to exactly one reviewer; per-file read-vs-skimmed coverage stated.
- (5) Reproducibility: all reviewers read-only, pinned to the baseline SHA; citations are SHA + path:line + symbol.
- (6) Severity thresholds defined; fixability bar per tier (P2 taste findings without safe-deletion/mechanical/test-proof are documented, not fixed).
- (9) Branch ancestry declared: stacked main ← audit03/p0 ← audit03/p1 ← audit03/p2, final combined gate on the p2 tip.
- (10) Verification upgraded to exact CI parity (verified against .github/workflows/pr.yml): fmt --check, clippy -D warnings, test --all-features, cargo audit, cargo deny, ShellCheck + bats for shell changes.
- (12) Frozen findings + live disposition ledger (99-disposition.md) with confirmed/refuted/fixed(SHA)/deferred states.

**Accepted with modification** — findings 2, 7, 11, 13:
- (2) The WIP commit lands on main — that was the user's explicit grill decision and stands. Accepted the refinement: secrets/generated-file review before committing, tracked files only, untracked audit artifacts excluded, baseline SHA recorded.
- (7) "Maximum security" rescoped to "maximum security within the documented threat model": controls judged against named assets/adversaries/trust boundaries (no same-UID theater); any accepted-risk change is an explicit documented threat-model edit in the same commit. This sharpens rather than reverses the user's "maximum security" grill decision.
- (11) Date cutoff removed: all currently-open advisories regardless of age; non-Cargo shipped components inventoried. Unreachable mandatory feeds mark the supply-chain section INCOMPLETE in the summary (upgraded from footnote).
- (13) Performance fixes now require measurement/profile/benchmark evidence; PTY/AMQ runtime paths get targeted macOS smokes. Modified: the Linux live-smoke leg is delegated to the existing CI matrix on push rather than run locally — this is a macOS workstation; a local Linux environment is out of scope for this engagement and the gap is recorded per-fix in the ledger.

**Rejected with reason** — findings 1, 8:
- (1) Rename to audit04: rejected. The docs/audits/ series is at 02, so 03 is the next number in that series, and the user explicitly specified docs/audits/audit03. The real issue — collision with the unrelated docs/plans/audits/audit03 plan series — is addressed by a prominent disambiguation note in PLAN.md and the report summary, plus cross-references where findings touch the session-settings surface.
- (8) Embargo/fast-track disclosure discipline: rejected as scoped. The report is a private in-repo artifact for the sole repo owner, not a public disclosure; SECURITY.md's disclosure policy governs external reporters. Report-then-fix ordering happens within one engagement (hours), so P0 remediation is not meaningfully delayed. P0s are still fixed first (tier order).

## Round 2 — Codex

Most round-1 findings are addressed, and both logged rejections are reasonable. Remaining material issues:

1. **Baseline can delete prior audits:** the moves currently appear as tracked deletions plus untracked replacement files, but [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:15) commits only tracked changes. **Fix:** explicitly stage both rename destinations, exclude only `PLAN.md`/the log, and verify the cached diff recognizes the moves.

2. **P0 severity is inflated:** [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:24) makes any CI-gate violation—including formatting or lint—P0. **Fix:** treat failed gates as merge blockers while deriving severity from the underlying defect.

3. **Commands are not actually CI-equivalent:** plain `cargo audit` omits CI’s `--deny warnings` and accepted-advisory ignores, while “ShellCheck over dux-amq” can miss the root installer and extensionless doctor script. **Fix:** use the exact [security workflow](/Users/siavash/Projects/dux-amq-setup/.github/workflows/test.yml:65) invocation and `make overlay-test`.

4. **“CI on push” is a wrong assumption:** branch pushes run tests/security, but fmt, Clippy, and overlay checks require a pull request; overlay push runs only on `main`. **Fix:** explicitly authorize/open draft tier PRs and wait for every check, or provide an equivalent Linux runner.

5. **Linux runtime verification remains missing:** existing CI does not exercise live PTY reconnect, real AMQ delivery, installer upgrades, or Linux-only TIOCSTI behavior. **Fix:** run affected smokes on an ephemeral Linux VM, or mark those portions incomplete rather than merely noting the gap.

6. **Remediation has no adversarial review:** candidate findings are refuted before reporting, but security-critical fix diffs receive only automated tests. **Fix:** require one independent read-only tier-diff review before stacking the next branch.

7. **External research is not reproducible:** the plan inventories advisories but does not require component versions, query dates, official URLs, or retained tool output. **Fix:** add that evidence table or raw artifact to each supply-chain subsystem report.

VERDICT: REVISE
### Claude's response (round 2)

All 7 accepted; every factual claim was verified against the repo before accepting.

- (1) Confirmed via `git status --porcelain`: audit01.md/audit02.md are tracked deletions with untracked replacement dirs (the user reorganized them mid-session). Baseline step now stages both rename destinations with the deletions, verifies the cached diff shows moves, and excludes only PLAN.md + PLAN-REVIEW-LOG.md. This finding prevented real data loss in the baseline commit.
- (2) CI-gate violation removed from the P0 definition; gates are merge blockers, severity derives from the underlying defect.
- (3) Confirmed in .github/workflows/test.yml:65 (`cargo audit --deny warnings --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2024-0384`) and Makefile (`overlay-test: overlay-shellcheck overlay-bats`). Plan now uses the exact CI invocations and `make overlay-test` everywhere (baseline health record + tier gates).
- (4) Confirmed: pr.yml is `on: pull_request` only; test.yml on push; overlay-ci on PR + push to main. Plan now pushes each tier branch and opens a draft PR, requiring all PR checks green before the tier is declared done. Draft PRs are review artifacts; merging stays with the user.
- (5) Accepted via the alternative Codex offered: Linux-only runtime behavior (TIOCSTI wake, installer upgrades, Linux PTY reconnect) is marked explicitly "incomplete (Linux runtime)" in the disposition ledger per affected fix. An ephemeral Linux VM is not part of this engagement.
- (6) Added an independent read-only adversarial review of each tier's full diff before the next branch stacks on it; findings feed back as fix commits on the same tier.
- (7) Added a reproducibility evidence table per supply-chain subsystem report: component + version, source URL, query date, retained raw tool output.

## Round 3 — Codex

All seven round-2 findings are addressed. Three new consistency issues remain:

1. **The report is not private once CI runs:** [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:28) calls it private, but line 33 pushes it through draft PRs to a [public GitHub repository](https://github.com/SiavZ/dux-amq-setup), exposing the full report while later-tier findings remain unfixed. **Fix:** keep the report out of pushed branches until the final remediation tip, or use a private security advisory.

2. **Stacked PR bases and final merge are undefined:** targeting every PR at `main` creates cumulative diffs; targeting each at its predecessor means merging the `p2` PR does not update `main`. **Fix:** use `p0→main`, `p1→p0`, `p2→p1` for tier review, then open one final `p2→main` integration PR.

3. **Linux verification claims conflict:** [PLAN.md](/Users/siavash/Projects/dux-amq-setup/PLAN.md:34) correctly says Linux runtime paths remain untested, while line 54 says Linux regressions surface through CI. **Fix:** limit the latter claim to compile/test-visible regressions and preserve the explicit runtime-incomplete status.

VERDICT: REVISE
### Claude's response (round 3)

All 3 accepted.

- (1) Verified: `gh repo view` reports the repo is PUBLIC. The round-1 rejection of the disclosure finding was based on a wrong premise (I treated the repo as private). Plan now keeps the report unpushed until the final audit03/p2 tip; tier PRs carry fix commits only; anything still exploitable at wrap-up is withheld and routed through a private security advisory.
- (2) PR bases declared: p0→main, p1→p0, p2→p1 for tier review, plus one final p2→main integration PR carrying the whole stack and the report.
- (3) Risks section reworded: CI catches Linux compile/test-visible regressions only; Linux runtime behavior stays explicitly "incomplete (Linux runtime)" in the ledger. No contradiction remains.

## Round 4 — Codex

All three round-3 findings are addressed. No new material problems found.

VERDICT: APPROVED

---
Converged after 4 rounds (3 REVISE, 1 APPROVED). Awaiting user sign-off before implementation.

---

## Act 3 — Build

Builder: Codex (gpt-5.6-sol, reasoning max, codex-cli 0.144.1). Claude verifies every phase.
Structural adaptation (logged): the spec is a phased engagement, so Codex runs once per phase (audit report → P0 → P1 → P2) in ONE resumed session (thread 019f514d-6f31-7af0-b62e-08847913a084); Claude verifies and commits between phases; MAX_FIX_ROUNDS=2 per phase.

Baseline: WIP committed on main as 3d52074 (audit01/02 preserved as R100 renames into per-audit dirs; only PLAN.md/PLAN-REVIEW-LOG.md excluded).

### Round 1 — Codex build (Phase A: audit sweep + report)

Codex produced docs/audits/audit03/ (9 files, 938 lines): 39 findings (9 P0 / 28 P1 / 2 P2), coverage 194/194 baseline files (83 read, 111 skimmed, 0 uncovered), supply-chain research COMPLETE (OSV querybatch over the exact 375-package Cargo.lock, 2026-07-11, evidence tables retained). No files outside docs/audits/audit03/ touched (verified via git status). Runtime ~1h18m.

### Claude's verdict (Phase A)

ACCEPTED. Verification performed:
- git status: zero tracked-file modifications — write-scope constraint held.
- 4/4 code spot-checks confirmed exact: P0-05 (git.rs:354 unwrap_or_default → destructive exclude overwrite), P0-06 (pty.rs Drop joins reader that only exits after Drop ends — hang with surviving descendant), P1-19 (u64::MAX quiet window inverted vs docs), P0-08 (render.rs:5677 byte-slice of user macro text).
- P1-13 cross-validated the hard way: my pre-change health record initially recorded cargo audit/deny/overlay-test as passing — a pipe bug (captured tail's exit code). Corrected record: cargo audit FAILS (RUSTSEC-2026-0190 anyhow, RUSTSEC-2026-0205 scc — exactly the two IDs Codex predicted from OSV), cargo deny FAILS (same), make overlay-test FAILS (SC2329 dead sha256_file in dux-amq-doctor — corroborates P1-05). fmt/clippy/test genuinely green at baseline.
- Health record (corrected) inserted into 00-summary.md replacing the placeholder.

Pre-existing baseline gate failures: cargo audit, cargo deny, make overlay-test. All other gates green.

### Round 2 — Codex build (Phase B: P0 fixes)

Codex fixed all nine P0s plus two gate-unblocking partials (P1-13 dep bumps: anyhow 1.0.103, serial_test 3.5.0 dropping scc/sdd; P1-05 slice: sha256_file invoked directly in dux-amq-doctor) across 17 files, +733/−200. One logged deviation: portable-pty exposes no reader cancellation handle, so P0-06 uses process-group KILL + 1s bounded join + detach fallback instead of true cancellation. Elapsed 36m37s.

### Claude's verdict (Phase B)

ACCEPTED. Full diff read (all 17 files). Fixes are faithful to each finding's proof direction; no out-of-scope changes; style matches surrounding code. Independent gate run: fmt/clippy/test/audit/deny all exit 0 (audit+deny newly green — the P1-13 bumps cleared both baseline failures); make overlay-test passes shellcheck (SC2329 cleared) and fails only on pre-existing GNU-only test assumptions on macOS (touch -d @epoch, /dev/tty) — verified present at baseline unmodified; authoritative Ubuntu run delegated to draft-PR CI. Committed as 11 per-finding commits 3bba098..830bc31 on audit03/p0; disposition ledger updated (9 fixed, P1-13 fixed, P1-05 partial). Independent adversarial tier-diff review launched per plan; push + draft PR gated on its verdict.

### Round 3 — independent adversarial review (Phase B P0 tier)

Reviewer verdict: ISSUES. 1 blocker + 3 should-fix + 4 actionable nits:
- B1 (BLOCKER): P0-02 not durably closed — reclaim NOREPLACE preserved both bodies, but claim() used plain fs::rename that overwrote the stranded .inflight body on the next scan tick. Message loss merely moved reclaim-time → claim-time. VERIFIED against src/amq_inject.rs:291.
- S1: pty Drop group-SIGKILLed child.process_id() even after the child was reaped (portable-pty still returns the pid); a recycled pid could kill another dux agent's process group.
- S2: purge-all bailed on one malformed row before purging anything — right-to-erasure (T6) availability regression.
- S3: four new purge bails printed DB-derived attacker-controlled paths unsanitized.
- N1 graceful-flush window, N3 doctor timeout dropped, N4 sed symlink detach, N2 weak tests.

### Claude's response + Round 4 (Codex fix round 1)

Verified B1 and S-items against code, then routed all (skipping only N5/N6/N7 as pre-existing/intended) to the same Codex session. Codex fixed across 13 files: claim() NOREPLACE + reclaim .expired/ quarantine (B1); try_wait-gated signalling + HUP→250ms→KILL escalation (S1/N1); build_plans_for_all collects failures + row-only plans + nonzero exit (S2); sanitize::for_terminal across all purge path surfaces (S3); doctor hash via external sha256sum under 'to' (N3); sed --follow-symlinks (N4); discriminating P0-05 test + render-level P0-08 test (N2). Independent gate rerun: all five cargo gates exit 0, shellcheck clean. Committed 1da919d..98f2b81 (7 per-item commits). Second independent adversarial re-review launched.

### Round 5 — independent re-review (Phase B P0 tier, fix round 1)

Reviewer verdict: CLEAN. B1 confirmed truly closed end-to-end (live claim path skip-and-retries on NOREPLACE Err; reclaim quarantines to .expired/ with dedup; release() is dead code with zero prod callers so no live loss). S1/S2/S3/N1/N2/N3/N4 all correct; no fresh problems. One non-blocking nit: release() dead-code path still used plain rename.

### Claude's response

Tier is CLEAN. Applied the one-line release() NOREPLACE nit anyway (message-durability path, cheap future-proofing under the maximum-security stance) with a fail-closed collision test — committed. All cargo gates green. P0 tier complete: pushing audit03/p0 and opening a draft PR (p0 → main) for the Linux CI legs.

### Round 6 — Codex build (Phase C: P1 tier, 28 findings)

Codex fixed all 28 P1 findings (P1-05 and P1-13 completed from their P0 partials) across 46 files, +2921/−1232. Highlights: new DUX2 authenticated AMQ envelope (receiver binding + base64 body preservation + atomic mkdir-nonce replay, replacing DUX1) for P1-04; per-migration single transaction (P1-15); blocking git/fs work moved to workers for stage/discard/browser/diff/config-save (P1-23); lifecycle persistence rollback/compensation (P1-27); atomic config writes (P1-28); backup worker actually wired + config-save flush on exit (P1-14); u64::MAX quiet-window semantics corrected (P1-19); provider CLI minimum-version floors (P1-12); installer checksum verification + fork-release default + pinned Claude Peers commit (P1-07/08/11); config-diff full typed coverage (P1-26). One tolerated deviation: GNU-tar reproducibility exec skipped locally (no GNU tar on macOS); structure assertions pass. Elapsed 1h36m.

### Claude's verdict (Phase C, pre-review)

Independent gate rerun: all five cargo gates exit 0, shellcheck clean. Spot-read P1-15 (clean single-tx), P1-04 DUX2 (receiver binding + base64 + atomic nonce — solid upgrade), P1-19 (semantics correct), P1-14 (shutdown-flag wired). Deferred per-finding commits until the independent adversarial review returns, because 46 files with heavy cross-finding sharing (src/cli.rs in 5 findings) means review-fix commits would tangle with initial ones. Full-tier adversarial review launched (focus: DUX2 crypto, worker-offload races, lifecycle rollback, atomicity rails).

### Round 7 — independent adversarial review (Phase C P1 tier)

Reviewer verdict: ISSUES. 1 blocker + 1 should-fix + 5 nits; large "verified CLEAN" section (DUX2 crypto canonicalization/base64/nonce all sound, P1-15/16/28/14/19/20/21/03/17 confirmed correct).
- B1 (BLOCKER, data loss): P1-27 cleanup force-deletes a PRE-EXISTING user branch on upsert failure. NewProject arm returns owns_worktree=true for the attach_existing case too; remove_worktree runs `git branch -D`. VERIFIED: git.rs:446 branch -D best-effort; workers.rs:1602/1663 attach_existing returns true; handler dispatches cleanup. The fix introduced the path.
- S1 (should-fix): async config-save snapshot-and-forget → memory/disk divergence on A-fails-then-B-succeeds interleave (regresses P1-16's goal).
- N1 empty-section auto-advance dropped; N2 u64::MAX exact-match only (large-but-not-MAX still holds); N3 config-save worker spawn .expect panics UI; N4 test-only 400ms sleep keyed on filename in prod write path; N5 per-diff SyntaxCache reuse lost (perf).

### Claude's response + Round 8 (Codex fix round 1)

Verified B1 and S1 against code, routed B1+S1+N1-N4 (N5 as judgement call) to the same Codex session. Held all P1 commits pending this fix so review-fixes land cleanly on shared files.

### Round 8 — Codex fix round 1 (Phase C P1 tier) — KILLED, Claude took over

The Codex resume for the P1 review fixes was killed mid-run (background task stopped). It had applied B1 (owns_branch threading) and S1 (field-level config-save delta), N3 (graceful config-worker spawn), N4 (proper #[cfg(test)] test-action seam) before dying, but left the tree with broken TEST code: input.rs:12380 referenced workers::delay_next_config_save without importing the workers module (cargo build passed since it skips tests; cargo test did not). Per the codex-build takeover rule (a failed/killed round → Claude finishes directly), Claude completed the remainder rather than re-delegating:

- Verified B1 complete + correct: NewProject arm returns owns_branch = !attach_existing; ForkSession returns true (its fork branch is new); threaded handler → dispatch_unpersisted_agent_cleanup → remove_worktree(delete_branch). Attach-to-existing-branch + upsert failure now removes only the worktree, preserving the pre-existing branch.
- Verified S1 complete: run_config_save_worker tracks last_requested per path; save_config_change re-reads disk and applies only the delta between previous and desired via apply_toml_delta. Wrote the reviewer-requested A-fails-then-B-succeeds test (config_save_divergence_tests) proving the failed edit's field does not resurrect on disk — this also consumed the dead fail_next_config_save helper.
- Fixed the kill damage: added `use crate::app::workers;` to the input.rs test module.
- N2 (Claude): replaced exact `== u64::MAX` with a decade threshold ALWAYS_DELIVER_QUIET_WINDOW so any large-but-not-MAX quiet window means always-deliver; updated the config doc; added a large-finite test.
- N1 (Claude): extracted section_after_git_file_op pure helper + 5 unit tests; the completion handler advances off an emptied stage/unstage section (fires only on explicit stage/unstage, not the 2s poll).
- N3/N4 confirmed already handled by the killed round (graceful spawn fallback; test seam not a filename sleep).
- N5 (perf, per-diff SyntaxCache): deferred — restoring cross-diff cache reuse off-thread needs an Arc<Mutex<SyntaxCache>> rework; recorded as a ledger deferral, not silently dropped.

### Round 9 — independent re-review (Phase C P1 tier, post-fix)

Reviewer verdict: ISSUES, but narrowly — all 7 items (B1, S1, N1-N5) verified CORRECT in code (owns_branch threading clean across all 3 cleanup call sites + both job arms; apply_toml_delta handles removed/unchanged/nested/type-change; N1 boundary + poll-path isolation; N2 threshold; N3 graceful spawn; N4 no prod scaffolding; N5 ponytail marker present). Single should-fix: the B1 blocker's actual property (attach-existing + upsert failure PRESERVES the pre-existing branch) had NO test — reverting owns_branch threading would fail nothing.

### Claude's response

Accepted. Added two direct git::remove_worktree unit tests (git.rs): preserves_branch_when_not_owned (the data-loss property — fails if owns_branch is reverted) and deletes_branch_when_owned (the fresh-branch cleanup still works). Both pass; fmt + clippy + full test suite green. Chose the direct unit test over a heavy App+worktree integration harness (lazy-correct, pins the exact property). P1 tier now fully verified and covered — proceeding to commit.

### Round 10 — independent review (Phase D P2 tier)

Reviewer verdict: CLEAN. Both P2 items verified: P2-02 deletion breaks no target (PurgeReport only in purge.rs; PurgeOutcome::Done still exercised); P2-01 display-width reuse correct (Line::width → unicode-width columns), word-break degrades gracefully on an over-wide single char (no loop/panic), discriminating tests present. One nit: a non-discriminating CJK wrap test (passed under both old and new at max_width=4).

### Claude's response — engagement complete

Fixed the nit: retuned the test to max_width=5 where display-width and scalar-count diverge, so it now catches a revert. Amended + force-pushed P2. All three tiers CLEAN.

## Final resolution

All 39 audit03 findings fixed across three stacked, independently-reviewed branches:
- audit03/p0 (PR #51 → main): 9 P0 + 2 gate unblockers. 2 review rounds; round 1 caught a blocker (AMQ message loss moved reclaim→claim time).
- audit03/p1 (PR #52 → p0): 28 P1. 2 review rounds; round 1 caught a data-loss blocker (attach-existing branch force-deleted on upsert failure) + a config-save durability divergence. Codex fix round was killed mid-run; Claude took over per the codex-build takeover rule and repaired the broken test code it left behind.
- audit03/p2 (PR #53 → p1): 2 P2. 1 review round, CLEAN after a test-quality nit.

Every tier: fmt + clippy -D warnings + test --all-features + cargo audit + cargo deny + shellcheck green locally; Linux/overlay legs delegated to PR CI. Pre-change baseline health record captured (audit + deny + overlay-test were RED at baseline on the two RustSec advisories and SC2329 — all now green). One deferral: N5 (per-diff SyntaxCache reuse), marked with a ponytail comment. Disposition ledger: 39 fixed, 0 confirmed-open.

## CI resolution (post-push)

Linux CI caught one regression my macOS-limited local run missed: the P1-11 bats test (`supply-chain-rails.bats` #65) parsed `CLAUDE_PEERS_REV` with a sed pattern that didn't handle the env-overridable `${VAR:-<sha>}` form, so it extracted an empty revision and failed the 40-hex assertion. The install.sh pin was correct; only the test's parser was wrong. Fixed on audit03/p1 (grep -oE extraction), rebased audit03/p2 (P2 fixes + report) onto the new p1 tip, force-pushed both. Ran the full local bats suite this time — only the known Linux-only flock finalize tests (29-33) remain red on macOS, which pass on Linux CI.

Final CI: all four PRs green — #51 (p0), #52 (p1), #53 (p2), #54 (p2→main integration) — 11/11 checks each, including Linux Test, Security (cargo audit + deny), and the shell/bats overlay leg. Engagement complete; branches await the user's merge decision.
