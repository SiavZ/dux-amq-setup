# Phase 01: Supply-chain hardening

> Maps to audit findings: P0-2, P2-7

## Goal
Eliminate the three unverified install steps: dux release tarball
(`grep -oP` scrape), AMQ `curl|bash`, and `npx -y skills add ...
>/dev/null 2>&1`. Every artifact pinned, sha256-verified against a
checksum committed here, and logged.

## Pre-conditions
- Phase 00 baseline green.

## Files to touch
- `dux-amq/install.sh` — modify all three install branches.
- `dux-amq/checksums/{dux-vX.Y.Z.sha256, amq-install-<commit>.sha256, skills-amq-<rev>.json}` — create.
- `dux-amq/README.md` — add "Supply chain trust boundary" section.

## Steps
1. Pin a known-good dux upstream tag (after Phase 06 merges, the latest).
   Compute and commit its hash:
   ```bash
   sha256sum dux-linux-amd64.tar.gz > dux-amq/checksums/dux-${TAG}.sha256
   ```
2. Replace the dux install branch:
   ```diff
   - TAG=$(curl -fsSL .../latest | grep -oP '"tag_name":"\K[^"]+')
   + TAG="${DUX_TAG:-v0.X.Y}"
   + EXPECTED=$(awk '{print $1}' "$HERE/checksums/dux-${TAG}.sha256")
   + TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
   + curl -fsSL -o "$TMP/dux.tar.gz" "https://github.com/.../${TAG}/dux-linux-amd64.tar.gz"
   + ACTUAL=$(sha256sum "$TMP/dux.tar.gz" | awk '{print $1}')
   + [[ "$ACTUAL" == "$EXPECTED" ]] || { warn "dux sha mismatch"; exit 1; }
   + tar -xzf "$TMP/dux.tar.gz" -C "$TMP" && install -m 0755 "$TMP/dux" "$LOCAL_BIN/dux"
   ```
   `mktemp` + `trap` is the canonical pattern; do not `cd -`.
3. Pin and verify the AMQ install script. Prefer **vendoring**: copy the
   pinned commit's `scripts/install.sh` into
   `dux-amq/vendor/amq-install-<sha>.sh`, run it from disk, never from the
   network. Fallback: download-then-verify-then-bash with checksum from
   `dux-amq/checksums/`. Tee output to `$STATE_ROOT/amq/install.log` —
   never `>/dev/null 2>&1`.
4. Replace `npx -y skills add`. Drop the redirect; pin commit; block
   postinstall; log:
   ```diff
   - npx -y skills add avivsinai/agent-message-queue -g -y >/dev/null 2>&1
   + npx --yes --ignore-scripts skills@<pin> add \
   +   "avivsinai/agent-message-queue#${SKILLS_REV}" -g -y \
   +   2>&1 | tee "$STATE_ROOT/amq/skills-install.log" || warn "skills add failed; see log"
   ```
   Note: if `skills` itself shells out to `npm i` without `--ignore-scripts`,
   file an upstream issue and document the residual risk.
5. README "Supply chain trust boundary": enumerate every third party that
   lands code (Patrick D'appollonio, avivsinai, Anthropic, OpenAI, Google,
   Microsoft VSCode Remote-SSH); revocation recipe
   (`rm -rf ~/.local/bin/{amq,dux,*-amq} /data/state/{amq,agents}`).

## Validation
- `shellcheck dux-amq/install.sh` clean.
- Force a sha mismatch (edit checksum file): `install.sh` exits 1.
- Clean GCE VM bootstrap: install logs show real npm output, not silenced.
- `cat $STATE_ROOT/amq/skills-install.log` contains the expected skill name.

## Acceptance criteria
- [ ] No `curl … | bash` pipes remain.
- [ ] All three downloads sha256-verified against `dux-amq/checksums/`.
- [ ] Stderr captured to logs (no `>/dev/null 2>&1` on installs).
- [ ] README "Supply chain trust boundary" enumerates signers + revocation.
- [ ] Forced sha mismatch fails the install reproducibly.

## References
- Audit P0-2, P2-7.
- Sigstore "A Safer curl|bash": https://blog.sigstore.dev/a-safer-curl-bash-7698c8125063/
- npm `--ignore-scripts`: https://docs.npmjs.com/cli/v10/using-npm/scripts#ignore-scripts
- GitHub Artifact Attestations: https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations
