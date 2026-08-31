# Phase 04 — Documentation integrity

**Track:** A (foundation) · **Parallel-safe with:** 01, 02, 03
**Depends on:** nothing · **Blocks:** 03 (work item 7 consumes the restored rotation procedure), 23

## Goal

Repair the reference graph. Production files — including an installer and a source
module — point at documentation that no longer exists, and the STRIDE table's entire
provenance column is dangling.

## Evidence

Commit `562419e` ("docs: retire completed audit01/02/03 implementation plans")
removed `docs/plans/audits/**` — 73 files.

> **Provenance note, stated plainly:** the deletions were already present in the
> working tree at the start of the rustport session; `562419e` is the commit that
> recorded them, made at the user's explicit direction. The consequence below is
> real regardless of who removed them.

Files still referencing the deleted tree, verified by `grep -rn`:

| Referrer | What breaks |
|---|---|
| **`dux-amq/install.sh:8`** | Points operators at `docs/plans/audits/audit01/01-supply-chain-hardening.md` for **how to recompute `DUX_SHA256` / `AMQ_SHA256` / `AMQ_BINARY_SHA256`**. The rotation procedure is unreachable exactly when every pin is stale (Phase 03). |
| **`SECURITY.md:51-69`** | Every STRIDE row's provenance column cites a phase number resolving to a deleted file. |
| **`SECURITY.md:157-161`** | Specifies a CI check `scripts/validate-threat-model.sh` comparing the STRIDE table against "phase files under `docs/plans/audits/`". No `scripts/` dir exists **and** the input tree is gone — the check is doubly dead. |
| **`src/watch/builtin.rs`** | A source file referencing deleted docs. |
| **`.github/CODEOWNERS:6,21`** | Ownership rules for paths that no longer exist. |
| **`dux-amq/tests/finalize-migration.bats`** | A test referencing the deleted tree. |
| **`docs/operations/encryption-at-rest.md:315`** | Dangling link. |
| **`dux-amq/README.md:197`** | Dangling link. |
| **`docs/audits/audit03/{00-summary,07-coverage-manifest}.md`** | ~74 rows certify review of files that no longer exist. |

`SECURITY.md:146-149` states that stale rows "must be either re-validated or removed
in the same PR that supersedes them." `562419e` removed the referents without
touching the referrers — the exact failure that rule exists to prevent.

Additionally: **`git show HEAD^:docs/plans/audits/audit02/artifacts/27-branch-protection.json`
is now the only record of what branch protection was actually applied** — history-only,
which is precisely the class of evidence that should not be deletable.

## In scope

Restoring operationally load-bearing content, repairing every referrer, and adding a
link checker so this cannot recur.

## Out of scope

- Rewriting the STRIDE table's content → Phase 23.
- Applying branch protection → Phase 02 (this phase preserves the *evidence*).

## Decision: restore selectively, not wholesale

Resurrecting all 73 implementation-plan files would restore a completed work log
nobody will read. Instead:

- **Restore as living operations docs** (into `docs/operations/`) the content that is
  a *procedure* rather than a *work log* — chiefly the supply-chain hash-rotation
  steps, the TIOCSTI verification method, and the branch-protection evidence artifact.
- **Repair the referrers** everywhere else, pointing at `docs/audits/` (the findings,
  which still exist) or at the new rustport plans.

## Work items

1. **Recover the hash-rotation procedure** from
   `git show 562419e^:docs/plans/audits/audit01/01-supply-chain-hardening.md` and
   publish it as `docs/operations/supply-chain-pin-rotation.md`, updated for the
   current pin set. Repoint `dux-amq/install.sh:8`.
2. **Recover the branch-protection evidence** from
   `git show 562419e^:docs/plans/audits/audit02/artifacts/27-branch-protection.json`
   into `docs/operations/` so the record is not history-only. Phase 02 supersedes it
   with applied config; keep the original as the "before" state.
3. **Recover the TIOCSTI verification procedure** (audit01 phase 07 / audit02 phase 13)
   into `docs/operations/`, since `dux-amq/install.sh` implements the tri-state and
   `tiocsti-detect.bats` pins it.
4. **Rewrite `SECURITY.md`'s provenance column** to cite `docs/audits/auditNN/…`
   (extant) rather than the deleted plan files, or to cite the new
   `docs/operations/` procedures where one exists.
5. **Resolve `SECURITY.md:157-161`.** Either write `scripts/validate-threat-model.sh`
   against a live input (the `docs/audits/` tree plus these plans) and wire it into
   CI, or delete the specification. A specified-but-nonexistent security check is
   worse than none — it reads as coverage that does not exist. **Recommendation:**
   write it; Phase 23 depends on the STRIDE table being verifiable.
6. **Fix `src/watch/builtin.rs`**, `.github/CODEOWNERS:6,21`,
   `dux-amq/tests/finalize-migration.bats`,
   `docs/operations/encryption-at-rest.md:315`, `dux-amq/README.md:197`.
7. **Fix `docs/audits/audit03/07-coverage-manifest.md:42-100+`** — ~74 rows certifying
   review of deleted files. Either repoint to the reviewed content's current home or
   mark the manifest closed with a note explaining the move.
8. **Add a markdown link checker to CI** covering relative links across `*.md`, plus a
   grep gate that fails if any tracked non-markdown file (`.sh`, `.rs`, `.yml`,
   `CODEOWNERS`) references a path that does not exist. This is the gate that would
   have caught `562419e`.
9. **Write `ARCHITECTURE.md`** at the repo root. Per matklad's precedent, a codemap —
   "a map of a country, not an atlas of maps of its states" — naming the important
   modules and the architectural invariants, deliberately *without* deep links, since
   links go stale. This complements the generated per-file trees (Phase 02), which
   give local navigation but cannot express invariants.
10. **Reconcile `CLAUDE.md`'s stale `state/` paragraph** (`CLAUDE.md:77`, echoed in
    `src/app/state/mod.rs:6-12`). It is wrong in three places: the "PTY map" was
    deleted (`state/runtime.rs:25-28`; agent PTYs now live in `SessionState::Live`),
    only `help_scroll` is actually in `UiState` (`left_scroll_offset` at `mod.rs:68`
    and `prev_scrollback_offset` at `:90` are still loose on `App`), and **there is no
    modal stack** — `ui.prompt` is a single slot (`state/ui.rs:16`) and
    `close_top_overlay` (`mod.rs:2077-2112`) is a hardcoded 4-level ladder that omits
    the macro bar. Describe what exists; Phase 21 changes it and updates this again.

## Acceptance criteria

- [ ] `docs/operations/supply-chain-pin-rotation.md` exists and `dux-amq/install.sh:8`
      points at it.
- [ ] Branch-protection evidence and TIOCSTI procedure recovered into `docs/operations/`.
- [ ] `grep -rn "docs/plans/audits" .` returns **zero** hits outside `.git/` and this
      plan set's own artifacts.
- [ ] Every STRIDE provenance citation in `SECURITY.md` resolves to an existing file.
- [ ] `scripts/validate-threat-model.sh` exists and runs in CI, **or** its
      specification is removed from `SECURITY.md`.
- [ ] Markdown link checker and the non-markdown path-reference gate run in CI and pass.
- [ ] `ARCHITECTURE.md` exists and describes the current module layout and invariants.
- [ ] `CLAUDE.md:77` and `src/app/state/mod.rs:6-12` describe the actual state layout.
- [ ] `docs/audits/audit03/07-coverage-manifest.md` has no dangling rows.

## Validation

```bash
grep -rn "docs/plans/audits" . | grep -v '^\./\.git/' | grep -v '^\./docs/plans/rustport/'
# expect: no output

# every relative link in every tracked markdown file resolves
ci/check-links.sh

# no tracked non-markdown file references a nonexistent path
ci/check-path-refs.sh

bats dux-amq/tests/finalize-migration.bats
cargo test --all-features
```

## Risks

| Risk | Mitigation |
|---|---|
| Restoring docs re-creates the clutter the deletion removed | Restore only procedures, not work logs; place them in `docs/operations/` where they are maintained, not in a plans tree |
| The link checker floods CI with pre-existing breakage | Land the checker and the fixes in the same PR; the referrer list above is complete as of 2026-08-31 |
| `ARCHITECTURE.md` goes stale like `BRANCH_PROTECTION.md` did | Keep it invariant-level and link-free by design; Phase 24 re-reads it as an acceptance step |

## References

- `artifacts/research-debt-ci-security.md` §F3
- `artifacts/research-app-monoliths.md` §C2 (the stale `state/` paragraph, with evidence)
- `artifacts/research-external-patterns.md` §2 (ARCHITECTURE.md precedent)
