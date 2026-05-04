# Branch protection — expected server-side configuration

Audit02 (P0-I bundle, P0-H, audit02 §27.3 spot-check, SECURITY.md cadence rule)
mandates the following branch protection on `main`. This file is the source of
truth; configure via `gh api` (recipe at the bottom of this file) or via the
GitHub UI (Settings → Branches → Protection rules → main).

## main

- **Require pull request before merging**: yes
  - Required approvals: 1 (CODEOWNERS-aware)
  - Dismiss stale approvals on push: yes
  - Require approval from CODEOWNERS: yes
- **Required status checks**:
  - `Format` (cargo fmt)
  - `Clippy` (cargo clippy -D warnings)
  - `Test` (cargo test --all-features) — Linux + macOS matrix
  - `Audit` (cargo audit --deny warnings) — added by Phase 07
  - `Deny` (cargo deny check) — added by Phase 07
  - `Shellcheck` (overlay shell scripts) — added by overlay-ci
  - `Bats` (overlay bats tests) — added by overlay-ci
  - `Strict mode (require branches up to date before merging)`: yes
- **Disallow force push**: yes (covers force-push to main + delete)
- **Disallow deletion**: yes
- **Require signed commits on main**: opt-in if/when GPG enrollment is in place
- **Require linear history**: no (we use merge commits for `audit02/integration`)
- **Lock branch**: no (push allowed via PRs only)

## Tag protection

- Pattern: `v*` and `dux-amq-v*`
- Restrict who can push: maintainers only (CODEOWNERS)

## Default workflow permissions

- Settings → Actions → General → Workflow permissions: **Read repository contents and packages permissions**
- Allow GitHub Actions to create and approve pull requests: **off**

## Configuration recipe (idempotent)

```bash
gh api -X PUT \
  -H "Accept: application/vnd.github+json" \
  /repos/SiavZ/dux-amq-setup/branches/main/protection \
  -f required_status_checks[strict]=true \
  -f 'required_status_checks[contexts][]=Format' \
  -f 'required_status_checks[contexts][]=Clippy' \
  -f 'required_status_checks[contexts][]=Test' \
  -f 'required_status_checks[contexts][]=Audit' \
  -f 'required_status_checks[contexts][]=Deny' \
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
