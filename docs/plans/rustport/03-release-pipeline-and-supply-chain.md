# Phase 03 — Release pipeline: make the fork ship its own code

**Track:** A (foundation) · **Parallel-safe with:** 01, 02, 04
**Depends on:** nothing · **Blocks:** 20 (installer delegation needs a real artifact to fetch)

## Goal

Fix the fact that this fork has never released the code it advertises, repair the
broken install path, and bring every supply-chain pin current with a rotation
procedure that is actually reachable.

## Evidence — all re-verified at the top level

- **All four `v*` tags are upstream ancestors.**
  `git merge-base --is-ancestor {v0.1.0,v0.2.0,v0.3.0,v0.4.0} upstream/main` → yes for
  all four. `git ls-tree --name-only v0.4.0 src/` contains **no** `peer.rs`,
  `amq_inject.rs`, `purge.rs`, `watch/`, `resume_recovery.rs`, `orphan_worktrees.rs`,
  `sanitize.rs`, `app/inject_runtime.rs`, `app/orchestrator.rs`, or `app/state/` —
  **~11,469 LOC of fork-only modules**, plus `input.rs` growing 10,506 → 13,266.
- **`api.github.com/repos/SiavZ/dux-amq-setup/releases/latest` returns HTTP 404.**
  Confirmed by direct request. The repo has exactly one release,
  `dux-amq-v0.1.0-rc1`, marked `prerelease: true`, and `/releases/latest` excludes
  prereleases. So `install.sh:97` dies with *"Could not determine latest release
  version."* **The documented default install path does not work today.**
- **`README.md` contains no path to install this fork** —
  `grep -n "SiavZ\|dux-amq-setup" README.md` is empty; `:19-45` points at
  `patrickdappollonio/tap/dux` and upstream URLs, while `:117,169-179,337` describe
  AMQ, `dux peer send`, and `dux doctor` as shipping features. `release.yml:238`
  extracts that same Install section into the fork's release notes.
- **`dux-amq/install.sh:220-231` falls back to downloading upstream `dux v0.4.0`** —
  correctly sha256-verified, for the wrong artifact, amd64-Linux-only and hardcoded
  at `:224`, therefore lacking every feature the overlay advertises. That fallback is
  exactly the new-machine path.
- **Every pin is stale**, and `dux-amq/install.sh:8` points at
  `docs/plans/audits/audit01/01-supply-chain-hardening.md` for the rotation
  procedure — a file that no longer exists (Phase 04 owns the doc repair).

| constant | pinned | latest (2026-08-31) | gap |
|---|---|---|---|
| `DUX_TAG` (`install.sh:34`) | v0.4.0 | v0.6.0 | 2 minors |
| `AMQ_TAG`/`AMQ_VERSION` (`:36-37`) | v0.61.0 | **v0.75.0** | **14 minors** |
| `SKILLS_PIN` (`:39`) | 1.5.3 | 1.5.23 | 20 patches, **no integrity hash** |
| `SKILLS_REV` (`:40`) | `ad3f934…` | = AMQ v0.61.0 commit | stale in lockstep |
| `CLAUDE_PEERS_REV` (`:41`) | `640183f…` | — | commit-pinned + `rev-parse`-verified — **correct, keep as the model** |

## In scope

Cutting a real release of fork code, fixing `resolve_version`, refreshing all pins
and their integrity hashes, README install path, and closing the release-pipeline gaps.

## Out of scope

- Dependency upgrades → Phase 01.
- Branch protection → Phase 02.
- Restoring the deleted rotation procedure doc → Phase 04 (this phase consumes it).
- Porting the installer to Rust → Phase 20.

## Work items

1. **Cut a real release from fork lineage.** Tag the current mainline, run
   `release.yml` against it, and verify the produced tarball contains `peer.rs`,
   `amq_inject.rs`, `purge.rs`, `watch/` and the rest of the fork-only set. Until this
   happens, no released artifact contains any Rust-side STRIDE mitigation.
2. **Publish it as a non-prerelease** so `/releases/latest` resolves. Alternatively —
   and better — **make `resolve_version` prerelease-tolerant**: fall back to
   `/releases` and take the newest entry when `/releases/latest` 404s, with a clear
   diagnostic. Do both; the fallback prevents a recurrence.
3. **Fix `install.sh` error handling** at `:89-99` so a 404 produces an actionable
   message naming `DUX_VERSION` rather than a bare failure.
4. **Rewrite the README install section** to install *this* fork. It currently
   advertises features (`dux peer send`, `dux doctor`, AMQ) that no documented install
   path can deliver. `release.yml:238` reuses this section, so fixing it fixes release
   notes too.
5. **Refresh every pin** in `dux-amq/install.sh:34-41` and recompute
   `DUX_SHA256`, `AMQ_SHA256`, `AMQ_BINARY_SHA256` against fresh downloads. Bump
   `AMQ_TAG` v0.61.0 → v0.75.0 (14 minors — read the changelog for wrapper-contract
   changes; the `--require-wake` / `--wake-inject-mode` / `--inject-via` flags the
   wrappers pass are AMQ surface and may have moved).
6. **Add an integrity hash for `SKILLS_PIN`.** It is the one pin fetched with no
   verification at all.
7. **Repoint `dux-amq/install.sh:8`** at the restored rotation procedure (Phase 04
   places it under `docs/operations/`).
8. **Change the upstream fallback at `:220-231`** to fetch the *fork's* release rather
   than upstream `dux v0.4.0`, and support both architectures rather than hardcoding
   amd64-Linux at `:224`. A fallback that silently installs a feature-incomplete
   binary is worse than a clear failure.
9. **Add a release-time smoke test**: install from the produced artifact into a clean
   container, run `dux --version` and `dux doctor`, assert the fork-only subcommands
   exist. This is the check that would have caught F1.
10. **Add an automated pin-freshness check** — a scheduled workflow that compares each
    pinned tag against the upstream latest and opens an issue on drift. 14 minors of
    silent drift is the failure this prevents.
11. **Preserve what already works.** The pipeline is mature: four targets with pinned
    OS versions, `cargo auditable build` (`release.yml:116`), `strip` (`:119`),
    CycloneDX SBOM (`:130`), double-package + `cmp` reproducibility (`:136-147`),
    `actions/attest-build-provenance` (`:154-158`), `SHA256SUMS` (`:186-218`). Do not
    regress any of it; the gap was never the pipeline's quality, only what it ran on.

## Acceptance criteria

- [ ] A non-prerelease GitHub release exists whose tarball contains the fork-only
      modules (verify by extracting and listing `src/`).
- [ ] `curl -fsSL .../releases/latest` resolves; the documented install command
      succeeds end-to-end on a clean Linux and a clean macOS host.
- [ ] `resolve_version` tolerates a prerelease-only repo and emits an actionable error.
- [ ] `README.md` documents installing this fork; `grep "SiavZ" README.md` is non-empty.
- [ ] All pins in `dux-amq/install.sh:34-41` current; all three sha256 constants
      recomputed and verified.
- [ ] `SKILLS_PIN` has an integrity hash.
- [ ] `dux-amq/install.sh:8` points at a file that exists.
- [ ] The no-`dux`-on-PATH fallback fetches fork artifacts for both arm64 and amd64.
- [ ] Release smoke test runs in CI and asserts fork-only subcommands are present.
- [ ] Pin-freshness workflow scheduled and opening issues on drift.
- [ ] SBOM, attestation, and reproducibility checks still pass on the new tag.

## Validation

```bash
gh release list
gh release view <new-tag> --json assets
curl -s -o /dev/null -w '%{http_code}\n' \
  https://api.github.com/repos/SiavZ/dux-amq-setup/releases/latest   # expect 200
tar tzf dux-linux-amd64.tar.gz | head
# in a clean container:
curl -fsSL https://raw.githubusercontent.com/SiavZ/dux-amq-setup/main/install.sh | bash
dux --version && dux doctor --json | jq -e '.versions'
bats dux-amq/tests/root-installer.bats dux-amq/tests/supply-chain-rails.bats
```

## Risks

| Risk | Mitigation |
|---|---|
| AMQ v0.61 → v0.75 changes the wrapper contract | Read the changelog for `wake`/`coop`/`drain`/`--inject-via` surface; `wrappers-p1.bats` pins the argv exactly and will fail loudly |
| A real release exposes code never shipped before | That is the point. Run the full suite plus the smoke test before tagging |
| `supply-chain-rails.bats` greps installer source text | It asserts literal lines in `install.sh`; update the test in the same commit as the pins |
| Recomputed hashes could pin a compromised artifact | Verify against the upstream release page and attestations, not just "what downloaded today" |

## References

- `artifacts/research-debt-ci-security.md` §F1, §F5, §Release pipeline
- `artifacts/research-bash-port.md` §8 (`dux-amq/install.sh`), §9 (root installer)
- `artifacts/bats-test-parity-matrix.md` §root-installer.bats, §supply-chain-rails.bats
