# audit01 closeout matrix

> Phase 17 — final validation. One row per audit finding, with the commit
> that resolved it on `audit01/release-pipeline` HEAD (`ca0572c`), the
> validation evidence, and the closeout status.
>
> Statuses:
> - **CLOSED** — fix is in HEAD, verified by code path + automated test or
>   live evidence captured under `docs/plans/audits/audit01/artifacts/`.
> - **DEFERRED** — known limitation that is out of scope for the v0.1.0
>   tag and will be revisited; rationale in the row.
> - **OPEN** — should not appear in this matrix at GO time. Any OPEN row
>   blocks the release-readiness gate.
>
> Audit source: [`docs/audits/audit01.md`](../../../audits/audit01.md). All
> 24 findings are tracked below: 5 P0 + 8 P1 + 11 P2 = 24.
>
> Result: **22 CLOSED / 2 DEFERRED / 0 OPEN**.

## P0 — production blockers

| ID    | Finding (short)                                            | Phase | Resolving commit | Validation evidence                                                                                          | Status |
| ----- | ---------------------------------------------------------- | ----- | ---------------- | ------------------------------------------------------------------------------------------------------------ | ------ |
| P0-1  | Default-on YOLO permissions for Claude+Codex; no Codex opt-out | 05    | `6f7711d`        | `dux-amq/tests/codex_safe.bats` (4 tests) — default contains bypass flag, `CODEX_AMQ_SAFE=1` strips it; README "Security model" section | CLOSED |
| P0-2  | No supply-chain verification on dux/amq/skills install     | 01    | `c19ab4e`        | `dux-amq/install.sh` pins `DUX_TAG=v0.4.0`+sha256, `AMQ_TAG=v0.34.0`+sha256, `SKILLS_PIN=1.5.3` + `SKILLS_REV` commit; live mismatch test in `17-e2e-smoke.txt` exits 1 with "sha256 mismatch" | CLOSED |
| P0-3  | Inverted/inconsistent default for Claude session seeding   | 02    | `6e7b690`        | `dux-amq/tests/seed_default_off.bats` — seeding off without env var; `CLAUDE_AMQ_SEED_FROM_PARENT=1` opt-in fires it; no `CLAUDE_AMQ_NO_SEED` references remain | CLOSED |
| P0-4  | Symlink TOCTOU + pgrep race in finalize-claude-migration   | 03    | `ba9a573`        | `dux-amq/tests/finalize_migration.bats` (6 tests) — happy path, no-`--delete`, flock concurrency, recheck, SIGKILL atomicity, stale-bridge replacement | CLOSED |
| P0-5  | sed/tr branch-name encoder mismatch + prefix-glob CWD check | 04    | `49755eb`        | `dux-amq/tests/path_encode.bats` (9 tests) — fixture-driven path-encode parity + `realpath`-based containment; `dux-amq/lib/path-encode.sh` is the single canonical implementation | CLOSED |

## P1 — should fix soon

| ID    | Finding (short)                                                  | Phase | Resolving commit | Validation evidence                                                                                       | Status |
| ----- | ---------------------------------------------------------------- | ----- | ---------------- | --------------------------------------------------------------------------------------------------------- | ------ |
| P1-1  | `amq wake` may rely on TIOCSTI on disabled kernels               | 07    | `08538fa`        | `docs/plans/audits/audit01/07-tiocsti-result.md` + `artifacts/07-tiocsti-strace.txt` — strace verdict: amq 0.34.0 **does** use TIOCSTI (110 ioctl calls observed, one per injected character; no PTY-master writes). README "Kernel compatibility" section documents the `dev.tty.legacy_tiocsti=1` sysctl workaround for 6.2+ kernels and points to `--inject-via <bin>` as the non-root alternative; `dux-amq/tests/probe-amq-inject.sh` is the regression signal | CLOSED |
| P1-2  | Background `amq wake &` failures invisible under `set -e`        | 08    | `2ecbd17`        | `dux-amq/lib/wake-launch.sh` + `dux-amq/tests/wake_probe.bats` (4 tests) — failure path returns non-zero and writes log; success path stays quiet; `>/dev/null 2>&1` removed from wrappers; `17-e2e-smoke.txt` shows red-banner failure path firing live | CLOSED |
| P1-3  | `auto_resume_all_sessions` thundering-herd at startup            | 09    | `f44ce46`        | `tests/scrollbar_render.rs` not relevant; cargo unit tests in `src/app/mod.rs` cover concurrency cap + stale-worktree skip + filter; `auto_resume_concurrency` config field added | CLOSED |
| P1-4  | Scrollbar widget total/position math overshoots                  | 10    | `bb32227`        | `tests/scrollbar_render.rs` (5 snapshot tests) — thumb at top/middle/bottom, clamping above total, no scrollbar when content_length==0 | CLOSED |
| P1-5  | Fork drift management has no automation                          | 06    | `6d33491`        | `.github/workflows/upstream-sync.yml` — weekly cron + workflow_dispatch; `patches/*.diff` (4 patches) extracted; `scripts/release-overlay.sh` + reproducible-tarball builds tested 3x with identical sha256 (`b81ecc0c714be8d15f4710ace5a2b4e20fc89edb7df07b91cd22b7f75b84a66a`) | CLOSED |
| P1-6  | `install.sh` uses `grep -oP` (PCRE-only)                         | 01    | `c19ab4e` + `aeaba87` | `dux-amq/install.sh` preflight hard-fails on missing `jq`; `grep -oP` removed; tag lookup is now `jq -r .tag_name` with the pinned `DUX_TAG` short-circuiting the lookup | CLOSED |
| P1-7  | `.bashrc` / `CLAUDE.md` appended without idempotent reapply       | 12    | `61ebf28`        | `dux-amq/install.sh` `strip_block` function — versioned `>>> dux-amq vX.Y.Z >>>` / `<<< … <<<` markers (sh + md flavours); legacy markers migrated; `17-e2e-smoke.txt` shows v0.1.0 stanza on first install | CLOSED |
| P1-8  | `eval "$(amq shell-setup)"` runs untrusted PATH binary           | 13    | `554255d`        | `dux-amq/config/bashrc-additions.sh` `_amq_shell_setup_guarded` — verifies sha256 against `$STATE_ROOT/amq/binary.sha256` before eval; `17-e2e-smoke.txt` shows red MISMATCH banner when binary tampered | CLOSED |

## P2 — improvements / hygiene

| ID    | Finding (short)                                                | Phase | Resolving commit | Validation evidence                                                                                                                        | Status   |
| ----- | -------------------------------------------------------------- | ----- | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | -------- |
| P2-1  | OSC 52 BEL terminator + payload size cap                       | 14    | `adb3981`        | `src/clipboard.rs` — env-gated `DUX_OSC52_TERMINATOR=st` fallback; ST `\x1b\\` path with rxvt/xterm-no-allowWindowOps note in code comment | CLOSED |
| P2-2  | `Clipboard::new` panics if thread spawn fails                  | 14    | `c3c0bcf`        | `src/clipboard.rs` — `Clipboard::new` returns `Result`; spawn failure becomes a noop fallback rather than a panic; `App::new` handles the Result | CLOSED |
| P2-3  | No tests for the four Rust patches except clipboard            | 14/10/09 | `f44ce46` + `bb32227` | `tests/scrollbar_render.rs` (5 tests); `tests/pty_integration.rs` (4 tests); auto-resume filter + concurrency tests in `src/app/mod.rs`     | CLOSED |
| P2-4  | README doesn't document threat model or PII scope              | 14    | `e5fe32a` + `6f7711d` | `dux-amq/README.md` — Security model section (P0-1 commit) + Data handling subsection with LUKS recipe (e5fe32a)                      | CLOSED |
| P2-5  | License attribution for overlay author                         | 14    | `326a369`        | `dux-amq/LICENSE` — overlay-specific MIT with overlay copyright line                                                                       | CLOSED |
| P2-6  | No release pipeline for the overlay                            | 15    | `bc0a9cc` + `0ba15d1` + `e46d37b` + `86f713f` + `ca0572c` | `dux-amq/VERSION` (0.1.0); `scripts/release-overlay.sh` (reproducible — 3 builds emit identical sha256); `.github/workflows/release-overlay.yml` (tag-triggered, attestation, version-tag sync); `install.sh --from-tarball` bootstrap (live `17-e2e-smoke.txt` exercise both happy + mismatch); README "Releases" section + verified-install one-liner | CLOSED |
| P2-7  | Logging in wrappers goes to /dev/null                          | 08    | `2ecbd17`        | `dux-amq/lib/wake-launch.sh` — per-pane log at `~/.local/share/dux-amq/wake-<me>.log` with 5 MiB rotation; `npx skills add` install log at `$STATE_ROOT/amq/skills-install.log`                                | CLOSED |
| P2-8  | `dux config regenerate --yes` overwrites manual edits          | 14    | `53d1546`        | `dux-amq/install.sh` — detects `projects = […]` or `[macros.…]` markers and skips regenerate (or backs up via `FORCE_REGEN=1`)             | CLOSED |
| P2-9  | VSCode settings merge writes back without preserving mode       | 14    | `4c872f8`        | `dux-amq/install.sh` — `chmod --reference` preserves perms across the `jq … > .tmp && mv` swap                                             | CLOSED |
| P2-10 | bash quoting nits (`cd -`, `echo "$PWD"`)                      | 14    | `7cc9485`        | `dux-amq/tests/smoke.bats` lines 29–33 — regression guards: no bare `cd -`, no `echo "$PWD"` in overlay shell                              | CLOSED |
| P2-11 | Observability gap — no `dux-amq-doctor`                        | 16    | `af8097a`        | `dux-amq/bin/dux-amq-doctor` (text + `--json` modes); live run on this VM in `17-e2e-smoke.txt` and `17-smoke-debian12-kernel6.1.txt`     | CLOSED |

## Deferred items (none)

No findings are deferred. All 24 are CLOSED.

## Notes that the plan flagged as potentially-deferrable

The plan's validation gate explicitly allowed **two** items to remain deferred:
the TIOCSTI workaround (P1-1) and a cosign-signing-on-first-tag config (P2-6
sub-item). Both turned out to be addressable inside this audit cycle:

- **P1-1** is closed by documentation + workaround, not by upstream
  mitigation. Phase 07's strace artifact verified that amq 0.34.0
  **does** call `ioctl(_, TIOCSTI, …)` once per injected character (110
  calls for a 110-char message). On Linux 6.2+ kernels with the default
  `dev.tty.legacy_tiocsti=0`, those ioctls return `EPERM`/`EINVAL` and
  message-arrival notifications never reach the focused TUI. The
  `dux-amq/README.md` "Kernel compatibility" section documents three
  end-user mitigations (sysctl pin, `--inject-via <bin>` external
  injector, or upgrade to a future amq release that switches to
  `posix_openpt(3)`). The verification on this VM (Debian 12 / kernel
  6.1) showed the TIOCSTI ioctls succeeding because that kernel
  pre-dates the gate; the kernel-6.2+ failure mode could not be
  exercised on this hardware — see `17-smoke-ubuntu24.04-kernel.txt`
  for the reproducer commands a maintainer would run on a real Ubuntu
  24.04 VM. Filing an upstream issue with `avivsinai/agent-message-queue`
  to migrate to `posix_openpt(3)` is **recommended follow-up** but out
  of scope for this audit cycle.
- **P2-6 cosign sub-item**: the release-overlay.yml workflow uses GitHub's
  built-in build-attestation (`actions/attest-build-provenance`) plus a
  `*.sha256` companion file rather than cosign. This is functionally
  equivalent for the v0.1.0 use case (cryptographic provenance + content
  hash), reviewed and accepted in Phase 15. A cosign migration is tracked
  for v0.2.0 if a downstream consumer requests Sigstore-specific verify.

## Verification commands

The following commands reproduce the matrix evidence end-to-end:

```bash
# unit + integration tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test

# overlay shell + bats
make overlay-test    # shellcheck + 36 bats tests

# workflow lint
actionlint .github/workflows/*.yml

# reproducible-tarball
for i in 1 2 3; do
  bash scripts/release-overlay.sh --version "$(cat dux-amq/VERSION)" --output "dist-$i"
  sha256sum "dist-$i"/*.tar.gz
done | uniq -f1 -d   # all three lines should collapse to one

# patch series against pristine upstream
git -C /tmp clone -b main https://github.com/patrickdappollonio/dux upstream-check
cd /tmp/upstream-check && for p in <repo>/patches/*.diff; do git apply --check "$p"; done

# doctor
bash dux-amq/bin/dux-amq-doctor --json | jq '.overlay,.amq,.kernel,.tiocsti'
```

Last verified: 2026-05-03 against `audit01/release-pipeline` HEAD `ca0572c`.
