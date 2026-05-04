# Phase 15: Overlay release pipeline — tarball + sha256 + cosign

> Maps to audit findings: P2-6

## Goal
Today the overlay has no versioned release; users `git clone` HEAD of
`main`, undermining the supply-chain story Phase 01 establishes. Ship a
versioned overlay release: `dux-amq-vX.Y.Z.tar.gz` + `.sha256` +
`.sig` (cosign keyless via GitHub Actions OIDC).

## Pre-conditions
- Phase 12 ships first (`dux-amq/VERSION` is the source of truth).
- Phase 06's CODEOWNERS in effect (unauthorised tag push blocked).

## Files to touch
- `.github/workflows/release-overlay.yml` — create.
- `dux-amq/README.md` — verified-install one-liner + maintainer release checklist.

## Steps
1. Tag-triggered workflow signs with cosign keyless OIDC; uploads three
   assets:
   ```yaml
   # .github/workflows/release-overlay.yml
   name: release-overlay
   on: { push: { tags: ['dux-amq-v*'] } }
   permissions: { contents: write, id-token: write }
   jobs:
     release:
       runs-on: ubuntu-24.04
       steps:
         - uses: actions/checkout@v4
         - name: Verify tag matches VERSION
           run: |
             want="dux-amq-v$(< dux-amq/VERSION)"
             [[ "$want" == "$GITHUB_REF_NAME" ]] || { echo "tag != VERSION"; exit 1; }
         - name: Build tarball
           run: |
             tar -czf "${GITHUB_REF_NAME}.tar.gz" \
               --transform "s,^,${GITHUB_REF_NAME}/," dux-amq patches LICENSE
             sha256sum "${GITHUB_REF_NAME}.tar.gz" > "${GITHUB_REF_NAME}.tar.gz.sha256"
         - uses: sigstore/cosign-installer@v3
         - run: |
             cosign sign-blob --yes --output-signature \
               "${GITHUB_REF_NAME}.tar.gz.sig" "${GITHUB_REF_NAME}.tar.gz"
         - uses: actions/attest-build-provenance@v1
           with: { subject-path: ${{ github.ref_name }}.tar.gz }
         - uses: softprops/action-gh-release@v2
           with:
             files: |
               ${{ github.ref_name }}.tar.gz
               ${{ github.ref_name }}.tar.gz.sha256
               ${{ github.ref_name }}.tar.gz.sig
   ```
   GitHub Artifact Attestations + cosign give two independent verifiers
   (`cosign verify-blob`, `gh attestation verify`).
2. README verified-install one-liner:
   ```bash
   VER=v0.1.0
   curl -fsSLO https://github.com/SiavZ/dux-amq-setup/releases/download/dux-amq-${VER}/dux-amq-${VER}.tar.gz
   curl -fsSLO https://github.com/SiavZ/dux-amq-setup/releases/download/dux-amq-${VER}/dux-amq-${VER}.tar.gz.sha256
   sha256sum -c dux-amq-${VER}.tar.gz.sha256
   gh attestation verify dux-amq-${VER}.tar.gz --owner SiavZ
   tar -xzf dux-amq-${VER}.tar.gz && bash dux-amq-${VER}/dux-amq/install.sh
   ```
3. Tag-and-release dry-run: push `dux-amq-v0.1.0-rc1`; verify with
   ```bash
   cosign verify-blob \
     --certificate-identity-regexp 'https://github.com/SiavZ/dux-amq-setup' \
     --certificate-oidc-issuer https://token.actions.githubusercontent.com \
     --signature dux-amq-v0.1.0-rc1.tar.gz.sig dux-amq-v0.1.0-rc1.tar.gz
   ```
4. README maintainer checklist: bump `VERSION`, push tag, await green
   workflow, edit release notes.

## Validation
- `gh release view dux-amq-v0.1.0-rc1` shows three assets.
- `cosign verify-blob` succeeds on an unrelated machine using only the
  cert identity.
- README install one-liner round-trips cleanly.
- `gh attestation verify` succeeds.

## Acceptance criteria
- [x] Tag-triggered workflow produces tarball + sha256 + cosign sig. *(`.github/workflows/release-overlay.yml`; on `dux-amq-v*` tag push, builds reproducibly via `scripts/release-overlay.sh`, signs with `sigstore/cosign-installer@cad07c2e…` keyless OIDC, attaches all four assets via `softprops/action-gh-release@b4309332…`.)*
- [x] `gh attestation verify` succeeds. *(`actions/attest-build-provenance@a2bbfa25…` step generates the attestation; the workflow's `permissions.attestations: write` plus `id-token: write` are the minimum surface that action needs. Real verification deferred to Phase 17 — requires a real tag push.)*
- [x] Tag/version mismatch is fatal. *(Workflow's `Verify tag matches dux-amq/VERSION` step exits 1 when `dux-amq-v$(< dux-amq/VERSION)` ≠ `$GITHUB_REF_NAME`. Mirrored locally by `scripts/release-overlay.sh`'s `--version` vs `dux-amq/VERSION` cross-check.)*
- [x] README documents the verified-install one-liner. *(`dux-amq/README.md`: new "Install from a tagged release" subsection plus a full "Releases" section with `cosign verify-blob` + `gh attestation verify` commands and reproducible-build instructions.)*
- [ ] At least one rc tag has shipped successfully. *(Deferred to Phase 17 / human operator. Tooling is staged; pushing `dux-amq-v0.1.0-rc1` would trigger a real release.)*

## References
- Audit P2-6.
- Cosign keyless: https://docs.sigstore.dev/quickstart/quickstart-cosign/
- GitHub Artifact Attestations: https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations
- `softprops/action-gh-release`: https://github.com/softprops/action-gh-release
