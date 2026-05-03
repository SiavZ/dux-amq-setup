# Phase 00: Preflight — verify assumptions, baseline, scaffolding

> Maps to audit findings: — (foundation for all other phases)

## Goal
Establish a known-good baseline before any production-readiness change lands.
Re-verify the audit's spot-checked facts, snapshot CI green-ness, and create
the `dux-amq/tests/` bats harness later phases depend on.

## Pre-conditions
- Clean working tree on `dux-amq-setup`.
- `cargo`, `bash`, `shellcheck`, `bats-core`, `jq`, `git` on PATH.
- `upstream` remote configured to `patrickdappollonio/dux`.

## Files to touch
- `dux-amq/tests/` — create.
- `dux-amq/tests/lib/setup.bash` — common bats helpers (tmp `$HOME`, fakes).
- `.github/workflows/overlay-ci.yml` — runs shellcheck + bats on PR.
- `tests/scrollbar_render.rs` — empty placeholder (filled in Phase 10).

## Steps
1. Re-confirm spot-checked facts in code: `codex-amq:27` unconditional
   YOLO; `claude-amq:11/26-27` doc/code mismatch; `finalize:25` `--delete`,
   `:27,:29` non-atomic; `install.sh:23` `grep -oP`.
2. Re-measure upstream drift:
   ```bash
   git fetch upstream
   git log HEAD..upstream/main --oneline | tee /tmp/upstream-drift.txt
   ```
   Audit said 1 commit; at plan authoring it is **7**. Save the file as
   the artifact Phase 06 references.
3. Snapshot baseline tests green:
   ```bash
   cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
   shellcheck dux-amq/install.sh dux-amq/wrappers/* dux-amq/scripts/*.sh
   ```
   Resolve any failure before proceeding.
4. Add `dux-amq/tests/lib/setup.bash` with the helpers Phases 02/03/04/12
   will use:
   ```bash
   setup_isolated_home() {
     export TEST_HOME=$(mktemp -d); export HOME="$TEST_HOME"
     export PATH="$BATS_TEST_DIRNAME/fakes:$PATH"
   }
   teardown_isolated_home() { rm -rf "$TEST_HOME"; }
   ```
5. Add minimal CI:
   ```yaml
   # .github/workflows/overlay-ci.yml
   name: overlay-ci
   on: [pull_request, push]
   jobs:
     shell:
       runs-on: ubuntu-24.04
       steps:
         - uses: actions/checkout@v4
         - run: sudo apt-get update && sudo apt-get install -y shellcheck bats jq
         - run: shellcheck dux-amq/install.sh dux-amq/wrappers/* dux-amq/scripts/*.sh
         - run: bats dux-amq/tests
   ```
6. Land as one PR `chore(audit01): preflight scaffolding`.

## Validation
- `gh pr checks` green on the preflight PR.
- `bats dux-amq/tests` exits 0.
- `cargo test` passes locally and in CI.

## Acceptance criteria
- [ ] Four spot-checked facts re-confirmed at HEAD.
- [ ] Drift count + short hashes recorded.
- [ ] `cargo fmt`, `clippy -D warnings`, `cargo test`, `shellcheck` green.
- [ ] `dux-amq/tests/lib/setup.bash` sources cleanly.
- [ ] `overlay-ci.yml` passes on PR.

## References
- `dux-amq-audit.md` lines 5–11 (drift was 1 at audit; verified 7 at plan).
- bats-core: https://bats-core.readthedocs.io/
