# Coverage manifest

Baseline: `3d5207486cfd3e75b96fc3ea550430f0439da998`  
Generated from: `git ls-tree -r --name-only <baseline>`  
Baseline files: **194**  
Status: **83 read (R), 111 skimmed (S), 0 not covered (N)**

Legend:

- **R — read:** the complete file or every behaviorally relevant section was inspected, with symbols/snippets opened at line level where applicable.
- **S — skimmed:** structure, headings, symbols/tests, generated/binary metadata, or historical claims were systematically inspected; targeted sections were opened when they intersected a candidate.
- **N — not covered:** no file has this status.

`PLAN.md` and `PLAN-REVIEW-LOG.md` were read as task-control evidence but are untracked at the baseline, so they are deliberately not added to the 194-file denominator.

| # | Status | Baseline path | Coverage focus |
|---:|:---:|---|---|
| 001 | R | `.github/BRANCH_PROTECTION.md` | expected server controls and required checks |
| 002 | R | `.github/CODEOWNERS` | ownership coverage |
| 003 | R | `.github/dependabot.yml` | update ecosystem/schedule |
| 004 | R | `.github/workflows/overlay-ci.yml` | permissions, pins, shell/Bats coverage |
| 005 | R | `.github/workflows/pr.yml` | matrix, gates, pins, security tools |
| 006 | R | `.github/workflows/release.yml` | permissions, tools, packaging, attest/upload |
| 007 | R | `.github/workflows/test.yml` | matrix, gates, pins, security tools |
| 008 | R | `.gitignore` | generated/state exclusions |
| 009 | R | `CLAUDE.md` | architectural and safety invariants |
| 010 | S | `Cargo.lock` | all 375 exact package records; targeted dependency chains/checksums |
| 011 | R | `Cargo.toml` | direct/dev dependencies, edition, profiles |
| 012 | R | `LICENSE` | distribution metadata |
| 013 | R | `Makefile` | local/CI parity and shell coverage |
| 014 | R | `README.md` | public behavior and operator contracts |
| 015 | R | `SECURITY.md` | trust boundary, STRIDE, accepted risks |
| 016 | S | `assets/dux-logo.png` | binary image metadata/presence |
| 017 | S | `assets/dux-screenshot.svg` | vector structure/content metadata |
| 018 | S | `assets/themes/dux_dark.toml` | theme keys and values |
| 019 | R | `deny.toml` | advisory/license/source policy and ignores |
| 020 | R | `docs/audits/audit01/audit01.md` | prior evidence and unresolved/regressed items |
| 021 | R | `docs/audits/audit02/audit02.md` | prior evidence and claimed fixes |
| 022 | R | `docs/contributing/schema-policy.md` | migration/schema contract |
| 023 | R | `docs/operations/encryption-at-rest.md` | shipped security/operations contract |
| 024 | R | `docs/operations/threat-model.md` | long-form threats and mitigations |
| 025 | S | `docs/plans/audits/audit01/00-preflight.md` | historical audit plan/evidence |
| 026 | S | `docs/plans/audits/audit01/01-supply-chain-hardening.md` | historical supply-chain plan |
| 027 | S | `docs/plans/audits/audit01/02-seeding-default-flip.md` | historical behavior plan |
| 028 | S | `docs/plans/audits/audit01/03-finalize-migration-safety.md` | historical migration plan |
| 029 | S | `docs/plans/audits/audit01/04-path-encoding-and-cwd-check.md` | historical path plan |
| 030 | S | `docs/plans/audits/audit01/05-codex-yolo-opt-out-and-threat-model.md` | historical provider security plan |
| 031 | S | `docs/plans/audits/audit01/06-upstream-sync-automation.md` | historical automation plan |
| 032 | S | `docs/plans/audits/audit01/07-tiocsti-verification.md` | historical terminal security plan |
| 033 | S | `docs/plans/audits/audit01/08-wake-startup-probe.md` | historical wake plan |
| 034 | S | `docs/plans/audits/audit01/09-auto-resume-concurrency.md` | historical concurrency plan |
| 035 | S | `docs/plans/audits/audit01/10-scrollbar-math-and-tests.md` | historical UI plan |
| 036 | S | `docs/plans/audits/audit01/11-jq-release-lookup.md` | historical release plan |
| 037 | S | `docs/plans/audits/audit01/12-versioned-config-inserts.md` | historical installer plan |
| 038 | S | `docs/plans/audits/audit01/13-amq-binary-pinning.md` | historical artifact plan |
| 039 | S | `docs/plans/audits/audit01/14-p2-bundle.md` | historical cleanup plan |
| 040 | S | `docs/plans/audits/audit01/15-overlay-release-pipeline.md` | historical release plan |
| 041 | S | `docs/plans/audits/audit01/16-doctor-tool.md` | historical doctor plan |
| 042 | S | `docs/plans/audits/audit01/17-final-validation.md` | historical validation plan |
| 043 | S | `docs/plans/audits/audit01/README.md` | historical index/status |
| 044 | S | `docs/plans/audits/audit01/artifacts/00-preflight-upstream-drift.txt` | retained historical raw output |
| 045 | S | `docs/plans/audits/audit02/00-preflight.md` | historical audit plan/evidence |
| 046 | S | `docs/plans/audits/audit02/01-wrapper-defaults.md` | historical wrapper plan |
| 047 | S | `docs/plans/audits/audit02/02-install-idempotency.md` | historical installer plan |
| 048 | S | `docs/plans/audits/audit02/03-sanitizer.md` | historical sanitization plan |
| 049 | S | `docs/plans/audits/audit02/04-ui-thread-workers.md` | historical UI-worker plan |
| 050 | S | `docs/plans/audits/audit02/05-pty-poison-and-reader-join.md` | historical PTY plan |
| 051 | S | `docs/plans/audits/audit02/06-gha-pinning.md` | historical Action plan |
| 052 | S | `docs/plans/audits/audit02/07-ci-security-gates.md` | historical gate plan |
| 053 | S | `docs/plans/audits/audit02/08-amq-message-auth.md` | historical authentication plan |
| 054 | S | `docs/plans/audits/audit02/09-tracing-migration.md` | historical logging plan |
| 055 | S | `docs/plans/audits/audit02/10-gdpr-purge.md` | historical purge plan |
| 056 | S | `docs/plans/audits/audit02/11-migration-safety.md` | historical migration plan |
| 057 | S | `docs/plans/audits/audit02/12-path-encoding-cwd.md` | historical path plan |
| 058 | S | `docs/plans/audits/audit02/13-tiocsti-mitigation.md` | historical terminal mitigation plan |
| 059 | S | `docs/plans/audits/audit02/14-sqlite-wal-integrity.md` | historical storage plan |
| 060 | S | `docs/plans/audits/audit02/15-auto-resume-concurrency.md` | historical concurrency plan |
| 061 | S | `docs/plans/audits/audit02/16-resource-limits.md` | historical resource plan |
| 062 | S | `docs/plans/audits/audit02/17-app-decomposition.md` | historical architecture plan |
| 063 | S | `docs/plans/audits/audit02/18-session-state-machine.md` | historical state plan |
| 064 | S | `docs/plans/audits/audit02/19-schema-versioning.md` | historical schema plan |
| 065 | S | `docs/plans/audits/audit02/20-doctor-tool.md` | historical doctor plan |
| 066 | S | `docs/plans/audits/audit02/21-macos-ci-portability.md` | historical portability plan |
| 067 | S | `docs/plans/audits/audit02/22-wrapper-p1-bundle.md` | historical wrapper bundle |
| 068 | S | `docs/plans/audits/audit02/23-rust-p1-bundle.md` | historical Rust bundle |
| 069 | S | `docs/plans/audits/audit02/24-p2-bundle.md` | historical cleanup bundle |
| 070 | S | `docs/plans/audits/audit02/25-encryption-at-rest.md` | historical operations plan |
| 071 | S | `docs/plans/audits/audit02/26-threat-model-docs.md` | historical threat-doc plan |
| 072 | S | `docs/plans/audits/audit02/27-final-validation.md` | historical validation plan |
| 073 | S | `docs/plans/audits/audit02/README.md` | historical index/status |
| 074 | S | `docs/plans/audits/audit02/artifacts/00-baseline-rev.txt` | retained historical raw output |
| 075 | S | `docs/plans/audits/audit02/artifacts/00-cargo-audit.txt` | retained historical raw output |
| 076 | S | `docs/plans/audits/audit02/artifacts/00-cargo-deny.txt` | retained historical raw output |
| 077 | S | `docs/plans/audits/audit02/artifacts/00-spot-check.md` | retained historical checks |
| 078 | S | `docs/plans/audits/audit02/artifacts/00-upstream-drift.txt` | retained historical raw output |
| 079 | S | `docs/plans/audits/audit02/artifacts/08-upstream-issue.txt` | retained historical issue evidence |
| 080 | S | `docs/plans/audits/audit02/artifacts/13-upstream-issue.txt` | retained historical issue evidence |
| 081 | S | `docs/plans/audits/audit02/artifacts/17-app-fields-baseline.txt` | retained historical architecture data |
| 082 | S | `docs/plans/audits/audit02/artifacts/25-doctor-followup.txt` | retained historical follow-up |
| 083 | S | `docs/plans/audits/audit02/artifacts/27-acceptance.txt` | retained historical raw output |
| 084 | S | `docs/plans/audits/audit02/artifacts/27-branch-protection.json` | retained server-setting snapshot |
| 085 | S | `docs/plans/audits/audit02/artifacts/27-clippy.txt` | retained historical raw output |
| 086 | S | `docs/plans/audits/audit02/artifacts/27-coverage.txt` | retained historical raw output |
| 087 | S | `docs/plans/audits/audit02/artifacts/27-final-validation.md` | retained historical validation |
| 088 | S | `docs/plans/audits/audit02/artifacts/27-fmt.txt` | retained historical raw output |
| 089 | S | `docs/plans/audits/audit02/artifacts/27-overlay.txt` | retained historical raw output |
| 090 | S | `docs/plans/audits/audit02/artifacts/27-release-build.txt` | retained historical raw output |
| 091 | S | `docs/plans/audits/audit02/artifacts/27-security-spot-check.txt` | retained historical raw output |
| 092 | S | `docs/plans/audits/audit02/artifacts/27-tests.txt` | retained historical raw output |
| 093 | S | `docs/plans/audits/audit02/artifacts/27-tiocsti-detection.txt` | retained historical raw output |
| 094 | S | `docs/plans/audits/audit02/artifacts/27-validation.sha256` | retained historical digest manifest |
| 095 | S | `docs/plans/audits/audit02/artifacts/cleanup-worktrees-after.txt` | retained cleanup snapshot |
| 096 | S | `docs/plans/audits/audit02/artifacts/cleanup-worktrees-before.txt` | retained cleanup snapshot |
| 097 | R | `docs/plans/audits/audit03/01-session-settings-modal.md` | baseline audit03 feature plan/claims |
| 098 | R | `dux-amq/README.md` | overlay behavior/install/operations contracts |
| 099 | R | `dux-amq/config/bashrc-additions.sh` | persistent environment and hash guard |
| 100 | R | `dux-amq/config/claude-md-additions.md` | installed agent instructions |
| 101 | R | `dux-amq/config/dux-config-changes.toml` | intended overlay config changes |
| 102 | R | `dux-amq/install.sh` | complete installer/control/data paths |
| 103 | R | `dux-amq/scripts/amq-receive-verify` | envelope parsing/HMAC/replay |
| 104 | R | `dux-amq/scripts/amq-secret-init.sh` | secret generation/permissions/idempotency |
| 105 | R | `dux-amq/scripts/amq-send-signed` | canonicalization/sign/send contract |
| 106 | R | `dux-amq/scripts/dux-amq-doctor` | all diagnostic sections/contracts |
| 107 | R | `dux-amq/scripts/dux-amq-inject-bridge` | verification/routing/queue writes |
| 108 | R | `dux-amq/scripts/encode-claude-project-dir` | path encoding contract |
| 109 | R | `dux-amq/scripts/finalize-claude-migration.sh` | migration gates/atomic swaps/roots |
| 110 | R | `dux-amq/scripts/install-gocryptfs.sh` | package/install/mount behavior |
| 111 | S | `dux-amq/tests/amq-auth.bats` | test names/fixtures; targeted auth cases |
| 112 | S | `dux-amq/tests/doctor.bats` | test names/fixtures; doctor contracts |
| 113 | S | `dux-amq/tests/encoder-fixtures.bats` | path corpus/expected encoding |
| 114 | S | `dux-amq/tests/fakes/.gitkeep` | empty fixture placeholder |
| 115 | S | `dux-amq/tests/fakes/amq` | fake command behavior |
| 116 | S | `dux-amq/tests/finalize-migration.bats` | migration failure/idempotency cases |
| 117 | S | `dux-amq/tests/fixtures/claude-paths.txt` | encoding fixture data |
| 118 | S | `dux-amq/tests/inject-bridge.bats` | routing/strict/queue cases |
| 119 | S | `dux-amq/tests/install-idempotency.bats` | repeat-install assertions/gaps |
| 120 | S | `dux-amq/tests/lib/setup.bash` | common test harness/fakes |
| 121 | S | `dux-amq/tests/smoke.bats` | installed-file smoke assertions |
| 122 | S | `dux-amq/tests/strip-block.bats` | managed-stanza cases |
| 123 | S | `dux-amq/tests/tiocsti-detect.bats` | kernel-mode detection cases |
| 124 | S | `dux-amq/tests/wrappers-p1.bats` | wrapper regression cases |
| 125 | S | `dux-amq/tests/wrappers.bats` | wrapper derivation/argv cases |
| 126 | S | `dux-amq/vscode/settings-additions.json` | settings fragment/JSON structure |
| 127 | R | `dux-amq/wrappers/claude-amq` | full launch/identity/wake policy |
| 128 | R | `dux-amq/wrappers/codex-amq` | full launch/identity/hook policy |
| 129 | R | `dux-amq/wrappers/gemini-amq` | full launch/identity/wake policy |
| 130 | R | `install.sh` | root release resolution/download/install |
| 131 | R | `rust-toolchain.toml` | toolchain/components/rationale |
| 132 | R | `src/amq_activity.rs` | activity counters/state |
| 133 | R | `src/amq_inject.rs` | queue scan/claim/recovery/expiry/tests |
| 134 | R | `src/app/components/button.rs` | layout/width/render/tests |
| 135 | R | `src/app/components/checkbox.rs` | layout/wrap/render/tests |
| 136 | R | `src/app/components/mod.rs` | component exports |
| 137 | R | `src/app/inject_runtime.rs` | routing, holds, phases, expiry, tests |
| 138 | S | `src/app/input.rs` | all symbol regions; targeted blocking/Unicode/config paths |
| 139 | S | `src/app/mod.rs` | state/constructor/event regions; worker wiring/lifecycle paths |
| 140 | S | `src/app/orchestrator.rs` | orchestrator state/command policy |
| 141 | S | `src/app/render.rs` | all render symbols; targeted Unicode/layout paths |
| 142 | S | `src/app/sessions.rs` | all session symbols; targeted settings/diff/lifecycle paths |
| 143 | R | `src/app/state/git.rs` | Git/session state ownership |
| 144 | R | `src/app/state/mod.rs` | state module exports |
| 145 | R | `src/app/state/runtime.rs` | runtime worker/AMQ/watch state |
| 146 | R | `src/app/state/ui.rs` | prompt/layout/input state |
| 147 | S | `src/app/text_input.rs` | editing/cursor primitives and tests |
| 148 | S | `src/app/workers.rs` | all worker symbols; targeted lifecycle/poller/backup paths |
| 149 | R | `src/auto_resume.rs` | queue/concurrency behavior/tests |
| 150 | R | `src/cli.rs` | command parsing/config/purge/doctor/reset paths |
| 151 | R | `src/clipboard.rs` | worker/channel/error paths |
| 152 | R | `src/config.rs` | typed config/default/migration/render/save/validation paths |
| 153 | R | `src/crash.rs` | panic/crash reporting |
| 154 | R | `src/diff.rs` | bytes/text/diff rendering/error paths |
| 155 | R | `src/editor.rs` | editor resolution/spawn |
| 156 | R | `src/git.rs` | all Git command/path/stat helpers |
| 157 | R | `src/io_retry.rs` | retry classification/backoff |
| 158 | R | `src/keybindings.rs` | parsing/normalization/lookup/conflict/tests |
| 159 | R | `src/lib.rs` | public module surface |
| 160 | R | `src/lockfile.rs` | process lock lifecycle |
| 161 | R | `src/logger.rs` | initialization/path/sanitization/rotation |
| 162 | R | `src/main.rs` | startup/order/error handling |
| 163 | R | `src/model.rs` | session/provider/state transitions/serialization |
| 164 | R | `src/peer.rs` | Claude Peers/AMQ routing and root discovery |
| 165 | R | `src/provider.rs` | config-only provider command contract |
| 166 | R | `src/pty.rs` | spawn/I/O/terminal env/shutdown/tests |
| 167 | R | `src/purge.rs` | complete plan/execute/redact/test paths |
| 168 | R | `src/purge_encoding.rs` | encoding validation/mapping/tests |
| 169 | R | `src/raw_input.rs` | raw terminal mode lifecycle |
| 170 | R | `src/sanitize.rs` | terminal/path/handle sanitization/tests |
| 171 | R | `src/statusline.rs` | command/status formatting |
| 172 | R | `src/storage.rs` | open/migrate/CRUD/backup/integrity/tests |
| 173 | R | `src/storage/migrations/0001_initial_schema.sql` | complete schema DDL |
| 174 | R | `src/storage/migrations/0002_session_state_v2.sql` | migration DDL |
| 175 | R | `src/storage/migrations/0003_session_settings.sql` | migration DDL |
| 176 | R | `src/storage/migrations/0004_session_sort_order.sql` | multi-statement migration DDL |
| 177 | R | `src/theme.rs` | theme loading/semantic colors/tests |
| 178 | R | `src/watch/builtin.rs` | built-in rule construction |
| 179 | R | `src/watch/engine.rs` | matching/scheduling/backoff/actions/tests |
| 180 | R | `src/watch/mod.rs` | watch exports |
| 181 | R | `src/watch/reset_time.rs` | all capture formats/arithmetic/tests |
| 182 | R | `src/watch/rule.rs` | rule types/defaults/validation |
| 183 | S | `tests/amq_inject_integration.rs` | integration scenarios/assertions |
| 184 | S | `tests/auto_resume.rs` | concurrency scenarios/assertions |
| 185 | S | `tests/git_portability.rs` | filename/platform cases |
| 186 | S | `tests/limits.rs` | limit/default scenarios |
| 187 | S | `tests/logger_jsonlines.rs` | structured logging assertions |
| 188 | S | `tests/pty_integration.rs` | PTY lifecycle scenarios; descendant gap |
| 189 | S | `tests/purge_integration.rs` | purge categories/dry-run/order; failure gaps |
| 190 | S | `tests/sanitize.rs` | control/handle/path corpus |
| 191 | S | `tests/session_state.rs` | state transition/serialization cases |
| 192 | S | `tests/storage_integration.rs` | CRUD/integrity/backup cases |
| 193 | S | `tests/storage_migrations.rs` | migration happy/idempotent cases |
| 194 | S | `tests/watch_engine_integration.rs` | watch end-to-end scenarios |

## Totals and exclusions

| Metric | Count |
|---|---:|
| In-scope baseline files | 194 |
| Read | 83 |
| Skimmed | 111 |
| Not covered | 0 |
| Explicitly excluded baseline files | 0 |

The non-goals are scope boundaries, not file exclusions: AMQ binary internals beyond the wrapper contract, live GitHub server settings, multi-tenant hardening, and merging. Every repository file that documents or invokes those boundaries was still covered.
