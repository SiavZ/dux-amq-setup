# Release readiness gate — dux-amq v0.1.0

> Phase 17. Every item below must be GREEN before a maintainer is
> permitted to run `git tag dux-amq-v0.1.0 && git push --tags`. The tag
> push is **not** part of this audit phase — it's the user's gating call.
>
> Verified against `audit01/release-pipeline` HEAD `ca0572c` plus the
> Phase 17 finalize-test flake fix on `audit01/final-validation`.

## Verdict: **GO**

All 11 must-pass items are GREEN. No blockers, no deferred audit findings.

## Must-pass checklist

### 1. All 24 audit findings closed

| Status   | Count | Notes |
| -------- | ----- | ----- |
| CLOSED   | 24    | See `CLOSEOUT.md` for the per-finding matrix and evidence |
| DEFERRED | 0     | The plan allowed up to 2 (TIOCSTI workaround + cosign first-tag); neither was needed |
| OPEN     | 0     |       |

**Verify:**
```bash
grep -c '| CLOSED' docs/plans/audits/audit01/CLOSEOUT.md
# expected: 24 (one per row)
```
Current value: **24**.

### 2. All bats tests pass

**Verify:**
```bash
bats dux-amq/tests
```
Expected: `36 tests, 0 failures`.

Current value: **36/36 PASS** across 5 consecutive runs (Phase 17 fix
landed at line ~105 of `dux-amq/tests/finalize_migration.bats` — pre-create
the lock file to remove a startup race in the concurrency test).

### 3. All cargo tests pass

**Verify:**
```bash
cargo test
```
Expected: `test result: ok. 715 passed; 0 failed; ...` plus the two integration
binaries `pty_integration` (4 ok) and `scrollbar_render` (5 ok).

Current value: **724 / 724 PASS**.

### 4. Clippy `-D warnings` clean

**Verify:**
```bash
cargo clippy --all-targets -- -D warnings
```
Expected: `Finished` with no warning lines.

Current value: **clean** (no warnings, no errors).

### 5. Shellcheck clean on overlay shell

**Verify:**
```bash
shellcheck dux-amq/install.sh dux-amq/wrappers/* dux-amq/scripts/*.sh
```
Expected: exit 0, no diagnostics.

Current value: **clean** (run as part of `make overlay-test`).

### 6. Actionlint clean on both overlay workflows

**Verify:**
```bash
actionlint .github/workflows/upstream-sync.yml .github/workflows/release-overlay.yml
```
Expected: exit 0, no diagnostics.

Current value: **clean** (also clean across all 6 workflows in `.github/workflows/`).

### 7. Reproducible tarball: same sha256 across three independent builds

**Verify:**
```bash
rm -rf dist-1 dist-2 dist-3
for i in 1 2 3; do
  bash scripts/release-overlay.sh --version "$(cat dux-amq/VERSION)" --output "dist-$i" >/dev/null
  sha256sum "dist-$i"/*.tar.gz
done
```
Expected: three identical sha256 hashes.

Current value:

```
b81ecc0c714be8d15f4710ace5a2b4e20fc89edb7df07b91cd22b7f75b84a66a  dist-1/dux-amq-v0.1.0.tar.gz
b81ecc0c714be8d15f4710ace5a2b4e20fc89edb7df07b91cd22b7f75b84a66a  dist-2/dux-amq-v0.1.0.tar.gz
b81ecc0c714be8d15f4710ace5a2b4e20fc89edb7df07b91cd22b7f75b84a66a  dist-3/dux-amq-v0.1.0.tar.gz
```

### 8. `--from-tarball` install with verified sha256 succeeds

**Verify:** see `artifacts/17-e2e-smoke.txt`. The test:

1. Serves `dist-1/dux-amq-v0.1.0.tar.gz` over local HTTP.
2. Runs `bash dux-amq/install.sh --from-tarball <url> --sha256 <correct>`
   inside an isolated `HOME` and `STATE_ROOT`.
3. Confirms exit 0 + post-install layout.
4. Re-runs with a corrupted sha256 and confirms exit 1 + "sha256 mismatch"
   diagnostic.

Current value: **happy=0, mismatch=1** as expected.

### 9. `dux-amq-doctor` reports green on this VM

**Verify:**
```bash
bash dux-amq/bin/dux-amq-doctor --json | jq '.overlay,.amq,.kernel,.tiocsti,.symlinks'
```
Expected: overlay version equals `$(cat dux-amq/VERSION)`, amq version
non-empty, no MISMATCH banner.

Current value:
```
"0.1.0"
"0.34.0"
"6.1.0-45-cloud-amd64"
"(sysctl absent — pre-6.2 kernel)"
{ "claude": "/data/state/claude", "agents": "/data/state/agents" }
```

### 10. All `patches/*.diff` apply cleanly against `upstream/main`

**Verify:**
```bash
git clone --quiet -b main https://github.com/patrickdappollonio/dux /tmp/upstream-check
cd /tmp/upstream-check
for p in <repo>/patches/*.diff; do
  echo "=== $p ==="
  git apply --check "$p"
done
```
Expected: all four patches `--check` clean (no output = success).

Current value: **all 4 patches clean** against pristine
`upstream/main` at `c2feab7`.

### 11. End-to-end smoke

**Verify:** see `artifacts/17-e2e-smoke.txt`. The test exercises:

1. `bash dux-amq/install.sh` against fresh `HOME=$(mktemp -d)` +
   `STATE_ROOT=$(mktemp -d)` — completes with `INSTALL_EXIT=0`.
2. Doctor reports overlay 0.1.0, amq 0.34.0, expected paths.
3. Supply-chain mismatch sim: corrupting `DUX_SHA256` → exit 1.
4. Pinned-binary tamper sim: appending a byte to
   `$STATE_ROOT/amq-bin/amq` causes the bashrc guard to print a red
   MISMATCH banner and skip `eval "$(amq shell-setup)"`.
5. `--from-tarball` install both happy and mismatch paths.
6. `wake_launch` helper produces the expected red-banner failure when
   no `/dev/tty` is available (and writes the per-pane log).

Current value: **all 6 sub-tests behave as expected**.

## Sign-off prerequisites

Before pushing the tag, the maintainer must:

1. Read `CLOSEOUT.md` and confirm 24/24 CLOSED (matches the matrix).
2. Read this gate (you are here) and confirm verdict is GO.
3. Decide whether to `git tag dux-amq-v0.1.0` and `git push --tags` —
   this audit phase deliberately does NOT push the tag.

The tag push will trigger `.github/workflows/release-overlay.yml`,
which: (a) re-asserts `dux-amq/VERSION` matches the tag, (b) runs the
reproducible-tarball script, (c) uploads the tarball + sha256 +
attestation as release assets.

The release workflow does **not** sign with cosign (per Phase 15
review); GitHub's `actions/attest-build-provenance` is the chosen
provenance mechanism for v0.1.0. If a downstream consumer asks for
Sigstore signing, that's a v0.2.0 follow-up.
