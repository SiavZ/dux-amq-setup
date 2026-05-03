# Phase 17: Final validation — E2E smoke, kernel matrix, release gates

> Maps to audit findings: all P0 / P1 / P2 (gating verification)

## Goal
End-to-end smoke on clean VMs to verify all 24 findings are addressed,
gating the first production tag `dux-amq-v0.1.0`.

## Pre-conditions
- Phases 00–16 merged.
- Clean GCE Ubuntu 24.04 LTS VM, empty `/data` attached.
- A second VM at Debian 12 to confirm portability.

## Files to touch
- `docs/plans/audits/audit01/CLOSEOUT.md` — per-finding closeout matrix
  (renamed from the original draft `17-validation-log.md` for clarity now
  that it is the canonical sign-off artifact).
- `docs/plans/audits/audit01/RELEASE-READINESS.md` — release-readiness
  gate with verification commands + current values.
- `docs/plans/audits/audit01/artifacts/17-e2e-smoke.txt` — full E2E
  install transcript on this VM.
- `docs/plans/audits/audit01/artifacts/17-smoke-debian12-kernel6.1.txt`
  and `17-smoke-ubuntu24.04-kernel.txt` — kernel-matrix smoke results.

## Steps
1. **Bootstrap Ubuntu 24.04**: attach disk at `/data`, install README
   prereqs (`curl jq tar git rsync shellcheck bats`), run the verified-
   install one-liner from Phase 15.
2. **Defaults**: `dux-amq-doctor --json | jq .overlay` shows version.
   `claude-amq` opens with `--dangerously-skip-permissions`; `CLAUDE_AMQ_SAFE=1`
   opens without. Same dance for `codex-amq` /
   `CODEX_AMQ_SAFE=1` /`--dangerously-bypass-approvals-and-sandbox`. Fresh
   worktree → no seeded jsonls (Phase 02); `CLAUDE_AMQ_SEED_FROM_PARENT=1`
   re-run seeds with stderr count visible.
3. **AMQ functional**: two panes `alice`/`bob`; `amq send alice "hello"`
   from bob; alice's `amq list` shows it; wake delivers per Phase 07.
   `tail ~/.local/share/dux-amq/wake-alice.log` shows real activity.
4. **Migration safety** (Phase 03): with claude running, finalize → exit 1;
   kill claude, retry → success. `kill -9` mid-migration → `~/.claude` is
   symlink or backup, never absent.
5. **Path encoding** (Phase 04): worktrees with hyphen/underscore/unicode →
   encoded session dir matches Claude Code's; no double dirs.
6. **Scrollbar** (Phase 10): ≥1000-line pane; End → bottom; Home → top.
7. **Auto-resume herd** (Phase 09): 8 sessions, `concurrency=2`, restart →
   spawn rate bounded; UI responsive.
8. **Supply chain** (Phase 01): edit one hex char in
   `dux-amq/checksums/dux-vX.Y.Z.sha256` → install exits 1 "tarball sha mismatch".
9. **Hash-pinned shell-setup** (Phases 13, 16): append a byte to
   `/data/state/amq-bin/amq` → new shell shows red mismatch banner;
   `dux-amq-doctor` reports MISMATCH.
10. **Debian 12**: install completes; `shellcheck` clean; doctor reports
    Debian kernel.
11. **Validation log**: `CLOSEOUT.md` with 1 row per finding
    (P0-1 … P2-11) → step + evidence.
12. **Cut tag** `dux-amq-v0.1.0` — **out of scope for this phase.** The
    audit phase prepares evidence; the maintainer reviews
    `RELEASE-READINESS.md` and decides whether to push the tag. Tag push
    triggers `release-overlay.yml` which ships tarball + sha + attestation.

## Validation
- `CLOSEOUT.md` has 24 rows, all CLOSED with evidence.
- `RELEASE-READINESS.md` lists 11 must-pass items, all GREEN.
- All 12 steps produced expected outcomes.
- `gh attestation verify dux-amq-v0.1.0.tar.gz` will be runnable post-release.
- `dux-amq-doctor --json` from the released environment shows no MISMATCH
  and no "missing" version strings.

## Acceptance criteria
- [x] Clean Ubuntu 24.04 install end-to-end (Docker user-space; kernel-6.2 gap documented).
- [x] Clean Debian 12 install end-to-end (this VM, kernel 6.1).
- [x] All P0 fixes verified live (1 row each in `CLOSEOUT.md`).
- [x] All P1 fixes verified live.
- [x] All P2 fixes verified live.
- [ ] Release tag pushed; assets cosign + attestation verified. (Pending — out of scope for this phase, see `RELEASE-READINESS.md`.)
- [x] No regression in `cargo test`, `cargo clippy -D warnings`, `bats`, `shellcheck`, `actionlint`.

## References
- Every prior phase in this directory.
- Audit `dux-amq-audit.md` (full document — checklist source).
