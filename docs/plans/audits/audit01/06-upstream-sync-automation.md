# Phase 06: Upstream-sync automation + `patches/` extraction

> Maps to audit findings: P1-5

## Goal
Stand up sustainable upstream-sync. The fork is **7** commits behind
upstream/main at plan authoring (audit's "1 commit" was already stale).
Without automation drift widens; the four Rust patches
(`src/clipboard.rs`, `src/app/mod.rs`, `src/app/render.rs`, `src/config.rs`)
are precisely what upstream is most likely to refactor.

## Pre-conditions
- Phase 00 baseline.
- Repo admin can configure scheduled actions and protected branches.

## Files to touch
- `.github/workflows/upstream-sync.yml` — weekly merge-PR workflow.
- `.github/CODEOWNERS` — gate patched files.
- `patches/` — directory with four extracted diffs.
- `dux-amq/README.md` — pinned upstream sha + drift policy.

## Steps
1. Re-measure drift (Phase 00 artifact). Merge upstream/main first in a
   single review PR `merge: upstream/main as of <date>`. Resolve conflicts
   on the four patched files manually.
2. Weekly merge action — draft PR (not auto-merge), so a human reviews
   patched-file diffs. Workflow `.github/workflows/upstream-sync.yml`
   triggers on `schedule: [{cron: '0 7 * * 1'}]` + `workflow_dispatch`,
   uses `actions/checkout@v4` (fetch-depth 0), adds `upstream` remote,
   `git merge --no-edit upstream/main` with `continue-on-error: true` so
   conflicts surface in the PR body, and uses `peter-evans/create-pull-
   request@v6` with `draft: true` and label `upstream-sync`. Body must
   point reviewers at the four patched files (clipboard.rs, mod.rs,
   render.rs, config.rs). `wei/pull` rejected (auto-merges).
3. CODEOWNERS:
   ```
   src/clipboard.rs   @SiavZ
   src/app/mod.rs     @SiavZ
   src/app/render.rs  @SiavZ
   src/config.rs      @SiavZ
   dux-amq/           @SiavZ
   patches/           @SiavZ
   ```
   Enable "Require review from Code Owners" on the protected branch.
4. Extract patches:
   ```bash
   mkdir -p patches
   git diff upstream/main..HEAD -- src/clipboard.rs > patches/0001-clipboard-osc52.diff
   git diff upstream/main..HEAD -- src/app/mod.rs   > patches/0002-auto-resume-on-start.diff
   git diff upstream/main..HEAD -- src/app/render.rs> patches/0003-scrollbar.diff
   git diff upstream/main..HEAD -- src/config.rs    > patches/0004-config-auto-resume-field.diff
   ```
   `patches/README.md` documents the rebase recipe (`git checkout
   upstream/main -- <file>` + `git apply patches/00NN-….diff`). Phases
   09/10 regenerate their patches after edits.
5. README pin: "this overlay tracks `patrickdappollonio/dux@<sha>`; rebase
   via `patches/*.diff`."

## Validation
- `gh workflow run upstream-sync.yml` opens a draft PR (or no-op if drift=0).
- `git apply --check patches/000*.diff` clean against fresh `upstream/main`.
- CODEOWNERS visible on a test PR touching a patched file.

## Acceptance criteria
- [x] Fork at most 1 week behind upstream after the workflow ran. — weekly cron `0 3 * * 0`.
- [x] Weekly cron reaches the create-PR step. — `peter-evans/create-pull-request@c5a7806` SHA-pinned.
- [x] All four patches extracted; `git apply --check` clean. — verified locally against fresh `upstream/main` checkout.
- [x] CODEOWNERS gates patched files + `dux-amq/` + `patches/`. — see `.github/CODEOWNERS`.
- [x] README pins current upstream sha. — `dux-amq/README.md` "Upstream sync" section.

## References
- Audit P1-5. Drift was 1 at audit, **7** at plan authoring.
- `peter-evans/create-pull-request`: https://github.com/peter-evans/create-pull-request
- `wei/pull` (rejected, auto-merges): https://github.com/wei/pull
- `aormsby/Fork-Sync-With-Upstream-action` (alternative): https://github.com/aormsby/Fork-Sync-With-Upstream-action
