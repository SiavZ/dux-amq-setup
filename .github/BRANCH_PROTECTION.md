# Branch protection: expected server-side configuration

Audit02 (P0-I bundle, P0-H, audit02 §27.3 spot-check, SECURITY.md cadence rule)
mandates the following branch protection on `main`. This file is the source of
truth; configure via `gh api` (recipe at the bottom of this file) or via the
GitHub UI (Settings → Branches → Protection rules → main).

## main

- **Require pull request before merging**: yes
  - Required approvals: 1 (CODEOWNERS-aware)
  - Dismiss stale approvals on push: yes
  - Require approval from CODEOWNERS: yes
- **Required status checks** (the contexts currently enforced on `main`;
  verify with `gh api /repos/SiavZ/dux-amq-setup/branches/main/protection`).
  These job names are kept stable on purpose. Upstream's workspace CI names
  its jobs `Test` and `Clippy and test (macOS)`; this fork renames them back so
  the protection that already exists keeps gating. Each job carries a
  `BRANCH-PROTECTION CONTRACT` comment. Do not rename them in an upstream merge.
  - `Test (ubuntu-24.04)`: `cargo test` on Linux, the `test` job in
    `.github/workflows/pr.yml` and `.github/workflows/test.yml`, pinned to
    `ubuntu-24.04`
  - `Test (macos-14)`: the `clippy-test-macos` job in the same two files.
    It runs clippy with `--all-targets` on `macos-latest` as upstream does
    (not yet proven on a macos-14 runner). `cargo test` is not run on macOS
    in CI yet, see the note at the end of that job
  - `Security`: `cargo audit` + `cargo deny check`, in pr.yml and test.yml
  - `shell`: `shellcheck` + `bats` from `.github/workflows/overlay-ci.yml`
  - `Strict mode (require branches up to date before merging)`: yes
- **Recommended (not currently required) but run on every PR**:
  - `Format`, `Clippy`, `Web lint`, `Web build and unit tests`,
    `dux-web dependency isolation`, `Reject DUX_DISABLE_UI_BUILD`, and
    `MSRV (1.88)` (`cargo +1.88.0 check`, which keeps the workspace's
    declared `rust-version` true).
    Add them to the protection if you treat them as merge gates; the recipe
    below shows the syntax.
- **Disallow force push**: yes (covers force-push to main + delete)
- **Disallow deletion**: yes
- **Require signed commits on main**: opt-in if/when GPG enrollment is in place
- **Require linear history**: no (merge commits are used for integration branches)
- **Lock branch**: no (push allowed via PRs only)

## Tag protection

- Pattern: `v*` and `dux-amq-v*`
- Restrict who can push: maintainers only (CODEOWNERS)
- Release tags for this fork are `dux-amq-vX.Y.Z`. The release workflow strips
  the prefix for the crate version and refuses to publish an archive that is not
  fork lineage (`.github/scripts/verify_fork_lineage.sh`).

## Default workflow permissions

- Settings → Actions → General → Workflow permissions: **Read repository contents and packages permissions**
- Allow GitHub Actions to create and approve pull requests: **off**

## Configuration recipe (idempotent)

```bash
# Apply the currently-enforced contexts. To also gate on Format/Clippy,
# add the matching `-f 'required_status_checks[contexts][]=...'` lines.
gh api -X PUT \
  -H "Accept: application/vnd.github+json" \
  /repos/SiavZ/dux-amq-setup/branches/main/protection \
  -f required_status_checks[strict]=true \
  -f 'required_status_checks[contexts][]=Test (ubuntu-24.04)' \
  -f 'required_status_checks[contexts][]=Test (macos-14)' \
  -f 'required_status_checks[contexts][]=Security' \
  -f 'required_status_checks[contexts][]=shell' \
  -f required_pull_request_reviews[required_approving_review_count]=1 \
  -f required_pull_request_reviews[dismiss_stale_reviews]=true \
  -f required_pull_request_reviews[require_code_owner_reviews]=true \
  -f enforce_admins=false \
  -f required_linear_history=false \
  -f allow_force_pushes=false \
  -f allow_deletions=false \
  -F restrictions=null
```

**Verify after applying:**
```bash
gh api /repos/SiavZ/dux-amq-setup/branches/main/protection | jq .
```
