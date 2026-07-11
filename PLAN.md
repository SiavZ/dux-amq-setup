# Plan: audit03 — full fresh sweep + autonomous remediation
_Locked via grill — by Claude + Siavash. Revised after Codex review round 1._

## Naming note

This is **audit03 in the `docs/audits/` series** (audit01.md, audit02.md → audit03/). It is unrelated to the `docs/plans/audits/audit03/` plan series ("Audit03 Phase 01 — session settings modal", already implemented and landed). The report summary will state this disambiguation explicitly, and cross-reference that plan where findings touch the session-settings surface.

## Goal

Run a from-scratch, full-coverage audit (audit03) of the entire dux-amq-setup working tree — the ~58k-line Rust `src/` tree, the `dux-amq/` shell/install/wrapper chain, CI/CD, and architecture — across all five dimensions used in prior audits: correctness, performance/efficiency, security, architecture/structure, and modernity. Prior audits (audit01/02) are input evidence but not the frame: every finding is re-derived against today's code, including the 11 files with uncommitted changes. The deliverable is a ranked findings report written into `docs/audits/audit03/` (multi-file: summary + per-subsystem), followed immediately by autonomous remediation of **all** tiers (P0→P1→P2) with a maximum-security stance, staged as stacked branches per tier.

## Approach

1. **Baseline**:
   a. Review the uncommitted WIP diff for secrets and stray generated files; commit the intentional WIP on `main` as a descriptive commit. The working tree currently shows `docs/audits/audit01.md`/`audit02.md` as tracked deletions with untracked replacement directories (`docs/audits/audit01/`, `audit02/`) — both rename destinations are staged together with the deletions so the prior audits are preserved as moves, and the cached diff is verified to show them as such before committing. Only `PLAN.md` and `PLAN-REVIEW-LOG.md` are excluded. Record the resulting baseline SHA — every audit citation pins to it.
   b. Capture a **pre-change health record** at the baseline SHA before any auditing, using the repo's exact CI invocations: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`, `cargo audit --deny warnings --ignore RUSTSEC-2025-0141 --ignore RUSTSEC-2024-0384` (kept in sync with `deny.toml`), `cargo deny --all-features check`, and `make overlay-test` (ShellCheck + bats, including root installer and extensionless scripts). Pre-existing failures are recorded in the report so they can't be misattributed to audit fixes.
2. **Coverage manifest**: enumerate every in-scope file (all of `src/` including `purge.rs`, `provider.rs`, `watch/`, all of `dux-amq/`, root installers/manifests, `.github/`, integration tests, `SECURITY.md` + `docs/operations/threat-model.md`). Every file is assigned to exactly one reviewer agent. The report states per-file coverage (read vs skimmed); nothing is silently dropped.
3. **Audit sweep** (parallel read-only agents + live web research):
   - Fan out reviewer agents by subsystem per the manifest: `src/app/` (input/render/sessions/workers/state), `src/` core (pty, storage, git, amq_inject, inject_runtime, config, tracing/sanitize, purge, provider, watch), `dux-amq/` (wrappers, install.sh, scripts, bats tests, root installers), CI/CD + supply chain (`.github/`, Cargo dependency tree), architecture/modernity (module structure, dead code, idioms, threat-model docs).
   - All reviewer agents are **read-only** and cite `baseline-SHA + path:line + symbol/snippet` for every finding.
   - Web research pass: **all currently-open** RustSec advisories for the exact `Cargo.lock` set (no date cutoff), GitHub Actions advisories, terminal-injection/agent-CLI CVEs; plus an inventory of shipped non-Cargo components (downloaded binaries, pinned installer hashes) checked against their upstream advisories. Each supply-chain subsystem report carries a **reproducibility evidence table**: component + version, advisory source URL, query date, and retained raw tool output (`cargo audit`/`cargo deny` JSON where available). If a mandatory feed is unreachable, the report's summary marks the supply-chain section **incomplete** — not a footnote.
   - Adversarial verification: every candidate finding gets a refute-style check before entering the report.
4. **Severity thresholds** (a finding is only *fixable* if it meets its tier's bar):
   - **P0**: exploitable security defect within the documented threat model, data-loss path, or panic/crash reachable in normal operation. (CI-gate failures are merge blockers handled in the fix workflow, but severity always derives from the underlying defect — a formatting violation is not a P0.)
   - **P1**: reproducible defect, measured or clearly-reasoned bottleneck on a hot path, violation of a CLAUDE.md tenet, or missing safety rail with a concrete failure scenario.
   - **P2**: modernity/structure improvements — only fixable when the change is a safe deletion, a mechanical idiom upgrade, or has test coverage proving no behavior change. Subjective taste without one of those is *documented, not fixed*.
5. **Security stance**: maximum security **within the documented threat model** (`SECURITY.md`: single-user, single-UID VM). Controls are evaluated against named assets, adversaries, and trust boundaries — no same-UID security theater. Where a fix tightens or contradicts a documented accepted risk, the fix updates `SECURITY.md`/`docs/operations/threat-model.md` in the same commit as an explicit, reviewed threat-model change. Workflow-affecting defaults (YOLO flags, auto-accept, seeding) become secure-by-default at full strength per the user's explicit instruction; every behavior change is listed prominently in the report summary.
6. **Report**: write `docs/audits/audit03/` — `00-summary.md` (executive summary, disambiguation note, ranked P0/P1/P2 table, coverage statement, pre-change health record, behavior-change list, audit01/02 cross-reference) plus one file per subsystem. Baseline findings are frozen evidence; a **disposition ledger** (`99-disposition.md`) is maintained through the fix phase recording each finding as confirmed / refuted-during-fix / fixed (with commit SHA) / deferred (with reason). **Publication timing**: the repo is public, so the report is written and versioned locally but **not pushed** while its findings are unfixed — tier PRs contain fix commits only. The report (with completed ledger) is committed and pushed on the final `audit03/p2` tip once remediation is done; any finding still open at that point that is exploitable-in-the-wild is withheld from the pushed report and handled via a private security advisory instead.
7. **Fix phase — fully autonomous, maximum security**:
   - **Stacked branches**: `main` (baseline) ← `audit03/p0` ← `audit03/p1` ← `audit03/p2`. Each tier branches from the previous tier's verified tip; the user merges `audit03/p2` to take everything, or a lower branch to take less. Commit per finding (or per tight cluster), referencing finding IDs.
   - Each tier must pass the **full CI-parity gate** before the next tier starts, using the exact CI invocations from step 1b (including `cargo audit --deny warnings` with the synced ignores, and `make overlay-test` when shell files changed). A final combined gate runs on the `audit03/p2` tip.
   - **Independent fix review**: before a tier is declared done and the next branch stacks on it, an independent read-only reviewer agent reviews the tier's full diff adversarially (correctness, security regressions, unintended behavior change). Findings feed back as fix commits on the same tier.
   - **Linux CI legs**: branch pushes trigger only `test.yml` (test + security matrix); fmt/clippy/overlay checks run on pull requests only. Therefore each tier branch is pushed and a **draft PR** is opened for it, and the tier is not declared green until all PR checks pass on GitHub. **PR bases**: `audit03/p0 → main`, `audit03/p1 → audit03/p0`, `audit03/p2 → audit03/p1` for per-tier review; after the final gate, one integration PR `audit03/p2 → main` carries the whole stack (plus the report). Draft PRs are review artifacts; merging remains the user's call.
   - Fixes to non-trivial logic include unit tests per CLAUDE.md. Performance fixes require evidence: a measurement, profile, or benchmark (micro-benchmark or timed test) demonstrating the claim — no speculative optimization. Runtime behavior that unit tests can't reach (PTY spawn/reconnect, AMQ delivery) gets a targeted local smoke on macOS. Linux-only runtime behavior (live TIOCSTI wake, installer upgrade paths, PTY reconnect under Linux) is **not** covered by existing CI or local smokes: the disposition ledger marks affected fixes' verification explicitly **incomplete (Linux runtime)** rather than merely noting a gap.
8. **Wrap-up**: final summary of what was found, fixed, refuted, and deferred; behavior changes highlighted; branches left for the user to merge.

## Key decisions & tradeoffs

- **Full fresh sweep, not a delta audit** — re-derives everything rather than triaging audit02's 62 items; costs more, but validates prior findings instead of trusting them.
- **Working tree as-is, not HEAD** — the audit covers uncommitted in-flight changes because that's what ships next.
- **WIP committed to `main` by Claude first** — explicit user decision from the grill (overrides PR-only convention for this baseline commit); secrets/artifact review happens before the commit.
- **Maximum security within the documented threat model** — secure fixes at full strength, including workflow-affecting defaults, but scoped to real trust boundaries; threat-model changes are explicit and documented, never silent.
- **Everything autonomous** — no sign-off gates between tiers; only truly irreversible decisions stop the run.
- **Stacked branch per tier** — review boundaries without merge ambiguity; one final gate on the stack tip.
- **Frozen findings + live disposition ledger** — baseline evidence never mutates, but implementation learnings (refuted/merged findings) are recorded with fix SHAs instead of preserving false claims.
- **Severity bars gate fixability** — P2 taste findings without a safety proof are documented, not auto-refactored; bounds regression risk on an autonomous run.

## Risks / open questions

- Web research quality depends on live source availability; unreachable mandatory feeds mark the supply-chain section incomplete in the summary.
- Findings in the 11 WIP files may be against code the user intends to rewrite anyway.
- P0 fixes requiring upstream AMQ coordination (e.g. peer auth) get an in-repo mitigation plus a documented upstream ask, not a fork of AMQ.
- ~58k lines + shell + CI is a large surface; the coverage manifest makes depth explicit per file rather than claiming uniform depth.
- Local verification runs on macOS; Linux-specific **compile- and test-visible** regressions surface via the PR CI matrix. Linux-only *runtime* behavior (TIOCSTI wake, installer upgrades, Linux PTY reconnect) is not exercised by any gate in this engagement and stays marked "incomplete (Linux runtime)" in the ledger.

## Out of scope

- AMQ binary internals (`avivsinai/agent-message-queue` Go source) beyond its interface contract with dux wrappers — separate engagement, consistent with audit02.
- Server-side GitHub configuration (branch protection, org policies) — documented as expected settings only.
- Multi-tenant hardening — the deployment model remains single-user-on-a-VM.
- Merging the fix branches to main — user's call after review.
