# Phase 02 — CI gates and modularity enforcement

**Track:** A (foundation) · **Parallel-safe with:** 01, 03, 04
**Depends on:** nothing · **Blocks:** 12–17 (the 500-line gate must exist before any split claims compliance)

## Goal

Make the modularity rules mechanically enforced rather than aspirational: a
file-length gate, a module-tree generator with a `--check` mode, orphan detection,
and — critically — branch protection that actually requires the checks the docs
claim are required.

## Evidence

- **CORRECTED 2026-08-31 (live API read, supersedes the audit02 artifact).** The
  audit02 evidence artifact
  (`git show 562419e^:docs/plans/audits/audit02/artifacts/27-branch-protection.json`)
  contains **no `required_status_checks` key**, which the research reported as "zero
  required checks were ever enforced." **That is no longer true of the live
  repository.** `gh api repos/SiavZ/dux-amq-setup/branches/main/protection` returns:

  ```text
  required_status_checks.strict:   true
  required_status_checks.contexts: Security, Test (macos-14), Test (ubuntu-24.04), shell
  required_pull_request_reviews:   1 approval, dismiss_stale: true,
                                   require_code_owner_reviews: true
  enforce_admins:      false        <- still a real gap
  required_signatures: false
  allow_force_pushes:  false
  allow_deletions:     false
  ```

  So four contexts **are** enforced and the review requirement is live and effective
  (verified: PR #56 was blocked with `mergeStateStatus: BLOCKED`,
  `reviewDecision: REVIEW_REQUIRED`). The audit02 artifact captured an earlier state.
- **The remaining real gaps** are narrower than reported: `Format` and
  `Clippy (ubuntu-24.04)` / `Clippy (macos-14)` are **not** in the required contexts
  — matching `.github/BRANCH_PROTECTION.md:21-26` but directly contradicting
  `CLAUDE.md:164`, which calls Clippy a gate that "fails the PR". And
  `enforce_admins: false` still means an admin can bypass everything, including the
  review requirement.
- `.github/BRANCH_PROTECTION.md:5` calls itself "the source of truth" while being a
  document that nothing verifies against the server — which is how it drifted out of
  date in the first place. Work item 8 replaces it with a drift check.
- **Duplicate check-run names:** `test.yml:13` and `pr.yml:66` both define a job named
  `Test (${{ matrix.os }})`. On a same-repo PR both report the same context against
  the same head SHA, so a required context can be satisfied by whichever run reports.
- **No Rust lint for file length exists.** `clippy::too_many_lines` is functions-only.
  [PR #16675](https://github.com/rust-lang/rust-clippy/pull/16675) proposes a
  file-level lint but is open with merge conflicts flagged 2026-08-01 and no approval.
- `cargo-modules` **0.27.0** (2026-08-03) provides `structure`, `dependencies
  --acyclic`, and `orphans --deny`. Reverse-dependency trees do **not** exist in any
  tool and must be written.
- Action pinning is already complete and correct — all 24 `uses:` lines pinned to
  40-char SHAs with version comments. Do not regress this.

## In scope

CI gates, the module-tree generator, branch protection as code, and the `#[allow]`
scorecard rule.

## Out of scope

- Applying the header convention to existing files → each Track D phase does this for
  the files it touches.
- Release-pipeline gates → Phase 03.

## Work items

1. **`ci/check-file-length.sh`** — fail if any tracked `.rs` exceeds 500 lines.
   Follow rustc's `src/tools/tidy/src/style.rs` precedent: a hardcoded limit, a clear
   error naming the file and its length, and a per-file opt-out directive
   (`// allow-long-file: <reason>` in the first 5 lines).

   ```bash
   #!/usr/bin/env bash
   set -euo pipefail
   MAX=${MAX:-500}
   fail=0
   while IFS= read -r f; do
     head -n 5 "$f" | grep -q 'allow-long-file' && continue
     n=$(wc -l < "$f")
     if (( n > MAX )); then
       printf '%s: %d lines (max %d)\n' "$f" "$n" "$MAX" >&2
       fail=1
     fi
   done < <(git ls-files '*.rs')
   exit $fail
   ```

   **Land it in warn-only mode initially** (22 files currently violate it), flip to
   blocking at the end of Track D. Phase 24 verifies the flip.
2. **`cargo modules orphans --deny --bin dux`** as a CI step. Invaluable during a
   ~140-file split: it catches source files that fell out of the module tree.
3. **`cargo modules dependencies --acyclic`** as a CI step — non-zero exit on a
   module dependency cycle.
4. **Write `ci/module-trees.rs`** (or `xtask`) — the generator that produces the
   `# Uses` / `# Used by` trees in file headers. Nothing off-the-shelf does reverse
   dependencies. Two viable bases: `syn` 3.0.4 for a parser-only pass over
   `use crate::…`, or `ra_ap_hir` 0.0.350 for name-resolved accuracy (zero-version,
   pin exactly). `dep_graph_rs` 0.2.0 is the closest existing thing — treat as a
   reference implementation to copy, **not** a dependency (58 recent downloads,
   no reverse-dep mode, ASCII output is a TODO).
   - Must emit `` ```text ``-fenced trees (see `CONVENTIONS.md` §1 — this is the rule
     that keeps them from being compiled as doctests).
   - Must support `--check` to diff generated vs committed and fail CI on drift.
5. **Enable `clippy::missing_docs_in_private_items`** (restriction group — covers
   private modules, which is most of a binary crate). rustc's `missing_docs` is
   public-items-only and therefore insufficient here.
6. **Fix the duplicate check-run names.** Rename `test.yml`'s job (e.g.
   `Test (push) (${{ matrix.os }})`) or restrict `test.yml` to non-PR branches, so a
   required context maps to exactly one workflow.
7. **Apply branch protection for real**, and commit it as code (a script or
   Terraform, not prose). Required contexts: `Format`, `Clippy (ubuntu-24.04)`,
   `Clippy (macos-14)`, `Test (ubuntu-24.04)`, `Test (macos-14)`, `Security`, `shell`,
   plus the new `file-length` and `module-trees` gates once flipped to blocking. Set
   `enforce_admins: true` and strict mode.
8. **Rewrite `.github/BRANCH_PROTECTION.md`** to describe the applied configuration
   and stop claiming to be the source of truth — the applied config is. Add a CI check
   that diffs the live GitHub API state against the committed config and fails on
   drift, so this cannot silently rot again.
9. **`#[allow]` scorecard.** 41 `#[allow]` attributes exist. Add a CI check requiring
   each to carry a `// reason:` comment on the preceding line, per `CLAUDE.md`. Audit
   the existing 41 and either justify or remove.
10. **Add a coverage job** (`cargo llvm-cov`). No coverage tooling is configured today,
    so "did the split drop tests?" is currently unanswerable. Report only — do not gate
    on a percentage.
11. **Wire a pre-commit hook** running `fmt`, the file-length gate, and
    `module-trees --check`, so contributors get the feedback before CI.

## Acceptance criteria

- [ ] `ci/check-file-length.sh` exists, runs in CI, and correctly reports the 22
      current violations.
- [ ] `cargo modules orphans --deny` and `dependencies --acyclic` run in CI and pass.
- [ ] The module-tree generator exists, emits `` ```text `` fences, and `--check`
      fails on drift.
- [ ] `clippy::missing_docs_in_private_items` enabled; `-D warnings` still exits 0.
- [ ] No two CI jobs share a check-run name.
- [ ] Branch protection **applied** with all listed contexts required, `enforce_admins:
      true`, verified by reading back the live API — not by reading the doc.
- [ ] A drift check compares live branch protection to committed config.
- [ ] All 41 `#[allow]`s carry a documented reason, or are removed.
- [ ] Coverage job reports a baseline number.
- [ ] Pre-commit hook documented in `docs/contributing/`.

## Validation

```bash
ci/check-file-length.sh || true                   # warn-only until Track D completes
cargo modules orphans --deny --bin dux
cargo modules dependencies --acyclic --bin dux
cargo run -p xtask -- module-trees --check
cargo clippy --all-targets --all-features -- -D warnings
gh api repos/SiavZ/dux-amq-setup/branches/main/protection   # must show required checks
```

## Risks

| Risk | Mitigation |
|---|---|
| Turning on branch protection blocks in-flight work | Apply after Track A lands; announce the required-context list first |
| `ra_ap_hir` has no API stability guarantee | Pin exactly; prefer `syn` if the parser-only pass proves sufficient |
| Generator produces churn on every refactor | That is the point — `--check` drift is a real signal. Regenerate as part of each Track D commit |
| `missing_docs_in_private_items` floods the build | Land it together with the header rollout, file-by-file, or scope it to new modules first |

## References

- `artifacts/research-debt-ci-security.md` §F4, §CI & supply chain, §In-code debt
- `artifacts/research-external-patterns.md` §2 (tooling that exists vs. must be written)
- `CONVENTIONS.md` §1, §2
