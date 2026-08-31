# Research brief — accumulated debt, deferred work, coverage, CI, supply chain, security

Captured 2026-08-31 at HEAD `562419e` ("docs: retire completed audit01/02/03 implementation plans"), branch
`shared-auto-resume` (19 commits ahead of `origin/main`, 0 behind).

Method: five parallel read-only sub-agents plus direct verification, with cross-checking where agents
disagreed. Every claim was checked against the source at HEAD, not read off a document. Where a document and
the code disagree, the code wins and the disagreement is recorded as a finding.

**Baseline health.** `cargo fmt --check` exit 0. `cargo clippy --all-targets --all-features -- -D warnings`
exit 0. `cargo test` exit 0, 1,139 distinct tests (1,056 unit + 83 integration; a raw 1,149 double-counts the
lib suite rerunning under the bin target). The 123 bats tests run in a separate CI job
(`.github/workflows/overlay-ci.yml:38`), not under `cargo test`.

**But the security gate is almost certainly red** — see S1 below. `cargo fmt`/`clippy`/`test` being green does
not mean CI is green.

---

## The five findings that reframe everything else

### F1. The fork has never shipped its own code, and its install path is broken today

All four `v*` tags are inherited upstream tags:
`git merge-base --is-ancestor {v0.1.0,v0.2.0,v0.3.0,v0.4.0} upstream/main` → YES for all four.
`git ls-tree --name-only v0.4.0 src/` contains no `peer.rs`, `amq_inject.rs`, `amq_activity.rs`,
`auto_resume.rs`, `crash.rs`, `lib.rs`, `orphan_worktrees.rs`, `purge.rs`, `purge_encoding.rs`,
`resume_recovery.rs`, `sanitize.rs`, `watch/`, `app/inject_runtime.rs`, `app/orchestrator.rs`, or
`app/state/` — **11,469 LOC of fork-only modules**, plus heavy edits to shared files (`input.rs` 10,506 →
13,266 since audit02). The two fork-lineage tags (`dux-amq-v0.1.0-rc1`, `audit02-validated`) both predate
`src/peer.rs`.

Consequences, each verified:

- `release.yml` has only ever run on upstream-lineage tags. Its reproducible tarballs, SBOMs, and attestations
  have never covered the fork's AMQ/peer/watch/purge/session-settings code.
- Every Rust-side STRIDE mitigation (T4, T6, T8, T9, T12–T18) lives in code no released artifact contains.
- **`api.github.com/repos/SiavZ/dux-amq-setup/releases/latest` returns HTTP 404.** The repo's only release is
  the prerelease `dux-amq-v0.1.0-rc1`, and `/releases/latest` excludes prereleases. So `install.sh:89-99`
  (`resolve_version`) dies at `:97` with "Could not determine latest release version." **The documented
  default install path does not work right now.**
- `README.md:19-45` points at `patrickdappollonio/tap/dux` and upstream release URLs. There is **no path in
  README.md to install this fork** (`grep -n "SiavZ\|dux-amq-setup" README.md` → empty), while
  `README.md:117,169-179,337` describe AMQ, `dux peer send`, and `dux doctor` as shipping features.
  `release.yml:238` extracts that same Install section into the fork's own release notes.
- `dux-amq/install.sh:220-231` falls back to downloading **upstream** `dux v0.4.0` when `dux` is not on PATH —
  sha256-verified against the right hash for the wrong artifact, amd64-Linux-only and hardcoded (`:224`), and
  therefore lacking every feature the overlay advertises. That fallback is exactly the new-machine path.

### F2. A parallel lineage holds finished audit01 work that never merged

`origin/dux-amq-setup` diverged at `554255d` (2026-05-03, the audit02 baseline) and never re-merged:
`git rev-list --left-right --count HEAD...origin/dux-amq-setup` → `224 37`.

Present there, absent at HEAD and on `origin/main`: `patches/0001-clipboard-osc52.diff` …
`0004-config-auto-resume-field.diff`; `.github/workflows/upstream-sync.yml`;
`.github/workflows/release-overlay.yml`; `scripts/bump-version.sh`, `scripts/release-overlay.sh`;
`dux-amq/LICENSE`; `dux-amq/VERSION`; `dux-amq/lib/path-encode.sh`, `dux-amq/lib/wake-launch.sh`.

Caveat against a naive cherry-pick: that branch predates the `dux-amq/bin|lib/` → `dux-amq/scripts/`
reorganisation, so recovery is a port, not `git cherry-pick`. The design work and tests exist regardless.

Related: 91 remote branches, **49 unmerged into `origin/main`**, including 7 `origin/audit01/*`. The pushed tag
`dux-amq-v0.1.0-rc1` has no workflow on mainline to service it.

### F3. HEAD deleted the evidence trail its own docs depend on

Commit `562419e` removed `docs/plans/audits/**` (73 files). **79 references remain**, now dangling:

- `SECURITY.md:157-161` — the planned CI check `scripts/validate-threat-model.sh` is specified to compare the
  STRIDE table against "phase files under `docs/plans/audits/`". No `scripts/` dir exists and the input tree is
  gone: the check is doubly dead.
- `SECURITY.md:51-69` — every STRIDE row's provenance column cites a phase number resolving to a deleted file.
- **`dux-amq/install.sh:8`** points operators at
  `docs/plans/audits/audit01/01-supply-chain-hardening.md` for the procedure to recompute `DUX_SHA256` /
  `AMQ_SHA256` / `AMQ_BINARY_SHA256`. **The documented hash-rotation procedure is now unreachable** — and every
  one of those pins is stale (F5).
- `docs/operations/encryption-at-rest.md:315`, `dux-amq/README.md:197`, `.github/CODEOWNERS:6,21`.
- `docs/audits/audit03/07-coverage-manifest.md:42-100+` — ~74 rows certify review of files that no longer exist.
- `git show HEAD^:docs/plans/audits/audit02/artifacts/27-branch-protection.json` is now the **only** record of
  what branch protection was actually applied — git-history-only, which is exactly the class of evidence that
  should not be deletable.

`SECURITY.md:146-149` says stale rows "must be either re-validated or removed in the same PR that supersedes
them." `562419e` removed the referents without touching the referrers.

### F4. Branch protection: the doc is aspirational, the artifact shows zero required checks

`.github/BRANCH_PROTECTION.md:5` calls itself "the source of truth" and claims four required contexts
(`Test (ubuntu-24.04)`, `Test (macos-14)`, `Security`, `shell`) plus strict mode.

The audit02 evidence artifact (`git show HEAD^:docs/plans/audits/audit02/artifacts/27-branch-protection.json`)
**contains no `required_status_checks` key at all** — only `required_pull_request_reviews` (1 approval,
dismiss-stale, CODEOWNERS), `allow_force_pushes: false`, `allow_deletions: false`,
`required_signatures: false`, `enforce_admins: false`. **Zero required status checks were ever enforced.**

On top of that:

- `Format` and `Clippy (…)` are explicitly listed as *not* required (`BRANCH_PROTECTION.md:21-26`), directly
  contradicting `CLAUDE.md:164`: "the `Clippy` check on every pull request runs this exact command and fails
  the PR if it emits any warning."
- **Duplicate check-run names.** `test.yml:13` and `pr.yml:66` both define a job named `Test (${{ matrix.os }})`.
  `test.yml` triggers on `push:` (any branch), `pr.yml` on `pull_request:`; on a same-repo PR both report the
  same context name against the same head SHA. A required context can be satisfied by whichever run reports.
- `enforce_admins: false` — combined with zero required checks, an admin can push anything to `main`.
- The file is a document, not configuration. Nothing verifies it matches the server.

### F5. Every artifact pin in the overlay installer is stale, and the rotation procedure was deleted

| constant | pinned | upstream latest (2026-08-31) | verdict |
|---|---|---|---|
| `DUX_TAG` (`dux-amq/install.sh:34`) | `v0.4.0` | **v0.6.0** (2026-05-25) | 2 minors, ~3 months |
| `AMQ_TAG` / `AMQ_VERSION` (`:36-37`) | `v0.61.0` | **v0.75.0** (2026-08-30, yesterday) | **14 minors** |
| `SKILLS_PIN` (`:39`) | `1.5.3` | **1.5.23** on npm | 20 patches; no integrity hash |
| `SKILLS_REV` (`:40`) | `ad3f934…` | = the AMQ v0.61.0 commit | stale in lockstep |
| `CLAUDE_PEERS_REV` (`:41`) | `640183f…` | — | commit-pinned + `rev-parse`-verified (`:356-360`) — good practice |
| `DUX_SHA256`/`AMQ_SHA256`/`AMQ_BINARY_SHA256` | — | — | correct hashes for stale artifacts |

And per F3, `dux-amq/install.sh:8` points at a deleted file for how to refresh them.

---

## Deferred-work ledger

Severity is severity **now**. Status: DONE / PARTIAL / NOT DONE / DOC-ONLY / REJECTED-BY-DESIGN / SUPERSEDED.

### Security gate and supply chain

| item | source | claimed | verified at HEAD | sev | what remains |
|---|---|---|---|---|---|
| **S1. RUSTSEC-2026-0253 (`lru 0.16.4`, unsound use-after-free) not ignored, not fixed** | fresh `cargo audit` against the committed lockfile | not tracked anywhere | **NOT DONE — CI gate is red** | **P0** | Reached via `ratatui-core 0.1.0` ← `ratatui 0.30.0`. Patched `>=0.18.2`. Absent from `deny.toml` and from the `--ignore` lists in `pr.yml:128-130` / `test.yml:73-75` |
| **S2. `chacha20 0.10.0` is YANKED** | same | not tracked | **NOT DONE — `cargo deny` fails** | **P0** | Reached via `rand 0.10.1` ← `petname 3.0.0`. `deny.toml:18` sets `yanked = "deny"`, so `cargo deny check` fails today |
| S1+S2 fix | — | — | — | — | **Both clear with a plain `cargo update`, no `Cargo.toml` edit**: `lru 0.16.4 → 0.18.3` (via `ratatui-core 0.1.0 → 0.1.2`), `chacha20 0.10.0 → 0.10.2` (verified with `cargo update --dry-run`) |
| **S3. RUSTSEC-2024-0384 rationale is false — upstream fixed it** | `deny.toml:31-36` | "waiting on `notify` to migrate" | **STALE** | **P1** | `notify-types 2.1.0` dropped `instant` for `web-time`; `notify 8.2.0` depends on `notify-types ^2`. The project is stuck only because `Cargo.toml:31` pins `notify = "7"`. Bump to 8 and delete the ignore |
| S4. RUSTSEC-2025-0141 (`bincode` via syntect) | `deny.toml:24-30` | ignored with rationale | **still accurate** | P2 | `syntect 5.3.0` (latest, 2025-09-27) still declares `bincode ^1.0` under the `default-syntaxes`/`default-themes` features dux selects (`Cargo.toml:26`). No upstream fix available |
| **S5. Advisory review date is 30 days overdue and its guard is a no-op** | `deny.toml:29,35`; `pr.yml:126`; `test.yml:71` | "Next review: 2026-08-01" | **EXPIRED** | **P1** | `dux-amq/tests/supply-chain-rails.bats:56-66` asserts the literal strings `RUSTSEC-2025-0141`, `RUSTSEC-2024-0384`, and `2026-08-01` are *present* — it never checks the date is in the future. The test that guards the deadline will pass forever |
| S6. Attestation produced, never verified | `release.yml:154`; audit03 P1-08 `fixed (605ca48)` | claimed fixed | **PARTIAL** | **P1** | `grep -i attest install.sh dux-amq/install.sh` → empty. `install.sh:141-142` verifies only a same-origin unsigned `SHA256SUMS`; anyone who can write a release asset rewrites both. `release.yml:150-151` even documents `gh attestation verify` and nothing calls it |
| S7. No `--locked` on any build/test | `pr.yml:30,63,88,128,133`; `test.yml:40,73,78`; `release.yml:116` | — | **NOT DONE** | P1 | `--locked` appears only on `cargo install` of tooling. `cargo auditable build` at `release.yml:116` may silently re-resolve semver-compatible deps, so the four release targets — built on four runners at four times — are not guaranteed to share a dependency set, and each CycloneDX SBOM describes whatever that runner resolved, not the committed lockfile |
| S8. `cargo set-version` mutates the tree at release time | `release.yml:106-110` | — | **by design, unsound** | P1 | Rewrites `[package] version` in `Cargo.toml` **and** the `dux` entry in `Cargo.lock` before the build. Every release builds from a tree matching no committed commit |
| S9. Reproducibility claim overstated | `release.yml:136-147` | "Package (reproducible)" | **PARTIAL** | P2 | `package()` is called twice over the *same already-built binary* and the tarballs `cmp`'d — that proves tar metadata determinism on one runner. The binary is never rebuilt, so **compile-level reproducibility is untested**. `SOURCE_DATE_EPOCH` is exported at `:121-122`, *after* the build at `:116`, so it can only affect tar mtime |
| S10. `sudo` escalation in the advertised curl-pipe | `install.sh:101-118,147,151` | — | **NOT DONE** | P1 | `resolve_install_dir` falls through to `/usr/local/bin` (`:117`) whenever `~/.local/bin` is absent or off PATH; `main` then runs `sudo install -m 755` (`:151`) after a check-then-use test at `:147`. The documented `curl … \| bash` flow can escalate to root without asking |
| S11. `bun install` unpinned, scripts enabled | `dux-amq/install.sh:363` | audit03 P1-11 fixed the *git rev* | **PARTIAL** | **P1** | The commit pin and `rev-parse` verification (`:356-360`) are sound. But `bun install` runs with no `--frozen-lockfile` and no `--ignore-scripts`, so third-party npm postinstall scripts execute — then `:366-367` registers the server as a **user-scoped MCP server for every Claude session on the box** |
| S12. `npx skills` has no integrity hash | `dux-amq/install.sh:315-317` | — | **NOT DONE** | P2 | `--ignore-scripts` and a version pin are present, but resolution is trust-on-first-use against the registry — unlike every other artifact this installer fetches, all of which get `verify_sha256` |
| S13. No `cargo vet` / `cargo crev` / `cargo semver-checks` / MSRV job | `.github/`, `Cargo.toml`, `Makefile` | — | **NOT DONE** | P2 | `Cargo.toml` has no `rust-version` field. For a project that compiles a six-month-old 16k-download crate into its binary (`opaline`), the absence of `cargo vet` is the notable one |
| S14. `security` job duplicated verbatim | `pr.yml:96-133`, `test.yml:45-78` | "keep both in sync" (`test.yml:44`) | **manual** | P3 | The `--ignore` lists are hand-copied with nothing enforcing parity |
| S15. Overlay release pipeline | audit01 P2-6; plan `audit01/15:83-87` unticked | plan unticked | **NOT DONE** | P1 | Exists as `release-overlay.yml` on `origin/dux-amq-setup` (F2) |
| S16. cosign signing | plan `audit02/07:161,194` | explicitly deferred | **NOT DONE** | P2 | Attestation-only, as the deferral accepted |
| S17. Upstream-sync automation + `patches/` | audit01 P1-5; plan `audit01/06:65-69` unticked | plan unticked | **NOT DONE** | P1 | `grep -rn "schedule\|cron" .github/workflows/` → zero. `upstream/main` ref last fetched 2026-05-30; real drift unmeasurable offline and ≥4 months |

### Code and architecture

| item | source | claimed | verified at HEAD | sev | what remains |
|---|---|---|---|---|---|
| **A1. `SessionState::transition` — the mandated lifecycle API — is never called** | `CLAUDE.md:87`; `src/model.rs:142,224` | "new lifecycle code should prefer `SessionState::transition`" | **DEAD** | **P1** | Measured by neutralizing the `#[allow(dead_code)]`s and running `cargo check --all-targets`: variant `Detached` and methods `can_transition_to`, `transition`, `detach`, `reattach` are all dead in the binary. Their only would-be callers (`find_pty_handle_mut`, `detach_session_pty`, `reattach_session_pty`, `src/app/mod.rs:3064`, ~80 lines) are dead too. The legal-edge enforcement the design rests on is inert. **Also: `CLAUDE.md:87` lists 5 variants; the code has 6 — `Retryable` is undocumented** |
| A2. `RemoteState` never created | plan `audit02/17:142` | "left for a follow-up" | **NOT DONE** | P1 | `grep -rn "RemoteState" src/` → zero hits. `ls src/app/state/` → `git.rs mod.rs runtime.rs ui.rs` |
| A3. State sub-structs carry no behaviour | plan `audit02/17` | phase 1+2 "landed" | **PARTIAL** | P1 | The only `impl` across all of `src/app/state/` is `impl Drop for RuntimeState` (`runtime.rs:152`). `UiState` and `GitState` have **zero** `impl` blocks. Fields moved; logic did not |
| A4. `App` field count ≤ 8 | plan `audit02/17:141` — "~130 leftover fields" | deferred | **PARTIAL — better than claimed** | P1 | `src/app/mod.rs:58-124` → **48 fields**, 3 of them sub-structs. Real 3× improvement; the "~130" is stale. Still 40 short |
| A5. `src/app/mod.rs` LOC reduction | plan `audit02/17:146` — baseline 2,727, target ~1,500 | deferred | **NOT DONE — regressed** | P1 | **4,447 LOC**, +53% since audit02 closed |
| A6. Module size generally | audit03 `06-architecture-modernity.md:29-41` | "not a fixable finding… no architecture cleanup epic is warranted" | **REJECTED-BY-DESIGN — and regressed** | P1 | Since 2026-07-11: `input.rs` 12,367→13,266, `render.rs` 7,239→7,565, `workers.rs` 2,058→**4,394**, `src/` 58,209→**69,515** (+19% in seven weeks) |
| **A7. UI-thread blocking — still violated after audit03 P1-23 "fixed"** | `CLAUDE.md`; audit03 P1-23 `fixed (773a6b0)` | claimed fixed | **PARTIAL** | **P1** | `src/app/inject_runtime.rs:367` runs `scan_queue_dir_limited` — a `read_dir` sweep of the **shared** `$AMQ_GLOBAL_ROOT` — plus `claim` (rename, `:438`), `read_validated` (`:453`), and quarantine renames (`:400,476`), all reached from `drain_events` (`workers.rs:790`) on the main loop (`mod.rs:1696`). A stalled mount freezes the TUI. `src/app/input.rs:3279` calls `git::branch_exists` — **two synchronous `git rev-parse` subprocesses** — inside the new-agent Enter handler, while every other git call in that file correctly uses `thread::spawn`. `src/app/workers.rs:27,62` run `git::ensure_project_worktrees_link` (`create_dir_all` + `canonicalize` + `symlink` + `.git/info/exclude` write) from `drain_events` |
| **A8. `keys.show_terminal_keys` is a documented setting that does nothing** | `src/keybindings.rs:1659` | — | **NOT DONE** | **P1** | Parsed (`config.rs:159`), defaulted true (`:854`), written to `config.toml` (`:2154`), reported by `dux config diff` (`cli.rs:1584`), stored into `RuntimeBindings` (`keybindings.rs:1709`) — **never read**. `config.rs:2492` tells the user "Set this to false to hide them." It has no effect. Violates the "config file is the documentation" tenet head-on |
| A9. Scrollbar `ScrollbarState` overcount | audit01 P1-4; plan `audit01/10:69-73` unticked; audit02.md:28 OPEN | plan unticked | **NOT DONE** | P2 | `src/app/render.rs:1394-1396` still `ScrollbarState::new(total + visible).viewport_content_length(visible)`, `position = total - offset` (`:1393`). Fixed form exists at `origin/dux-amq-setup:src/app/render.rs:1377-1378` |
| A10. Scrollbar snapshot tests | plan `audit01/10:72`; audit02 P2-19 | plan unticked | **NOT DONE** | P2 | `tests/scrollbar_render.rs` never existed on this lineage |
| A11. `Clipboard::new` → `Result` | audit01 P2-2 / audit02 P2-13 | plan unticked | **NOT DONE** | P2 | `src/clipboard.rs:28` returns `Self`; `:36` `.expect("failed to spawn clipboard worker thread")` |
| A12. `DUX_OSC52_TERMINATOR=ST` | audit01 P2-1 | plan unticked | **NOT DONE** | P3 | `src/clipboard.rs:124-125` hardcodes BEL; test `:249-253` pins it |
| A13. jq VS Code merge preserves mode | audit01 P2-9 | plan unticked | **NOT DONE** | P3 | `dux-amq/install.sh:532` — `> "$f.tmp" && mv "$f.tmp" "$f"` |
| A14. Display-width class only partly fixed | audit03 P2-01 `fixed (b13df41)` | claimed fixed | **PARTIAL** | P2 | Fixed in `button.rs:32-35`, `checkbox.rs:179`. But **24 `chars().count()` remain in `render.rs`** vs 8 `.width()` (36 vs 12 across `src/`), incl. `render.rs:2310-2311` and the second macro preview at `:6332-6343`. Char-boundary-safe (no panic) but wrong for CJK/emoji. **The cited SHA `b13df41` is unreachable from any branch** — the real commit is `7d715a0` |
| A15. Per-diff `SyntaxCache` | audit03 N5 `deferred (perf)` | deferred | **NOT DONE (intentional)** | P3 | `src/app/workers.rs:3102`, well-scoped `// ponytail:` marker naming cause, cost, and fix |
| A16. Legacy `crate::logger::` shims | `CLAUDE.md:147`; plan `audit02/09` all ticked | plan claims done | **PARTIAL** | P2 | **50 call sites** remain (44 short-form `logger::`, 6 fully-qualified `crate::logger::`) vs 113 `tracing::` macros. Concentrated in `app/sessions.rs`, `app/workers.rs`, `app/mod.rs`, plus `pty.rs:510`, `storage.rs:175`, `theme.rs:260`. The plan's bar was "≥5 hot-path sites" and was met; the structure loss matters because `purge.rs` log redaction consumes those fields |
| A17. Three implementations of the Claude path encoder | audit01 P0-5 / audit02 Phase 12 | claimed done | **PARTIAL** | P1 | `dux-amq/scripts/encode-claude-project-dir:17-22` (bash), `src/purge_encoding.rs:52-80` (fallible), and a private duplicate at `src/resume_recovery.rs:746-761` (infallible, `to_string_lossy`, no absolute check). Only the first two share `dux-amq/tests/fixtures/claude-paths.txt`; the third is pinned by one hardcoded case (`resume_recovery.rs:1086`). `purge_encoding::encode_claude_project_dir` is `#[allow(dead_code)]` **and measurably dead** |
| A18. Two implementations of handle normalisation | audit02 P1-2 / PLAN.md §B | claimed done | **PARTIAL** | P1 | `dux-amq/wrappers/claude-amq:163-195` vs `src/sanitize.rs:61-79` + `src/model.rs:730-748`. Must stay byte-identical; no shared fixture pins them |
| A19. Shared wrapper library | audit01 P0-5; plan `audit01/04:72` | plan unticked | **NOT DONE** | P2 | `dux-amq/lib/` absent. Three wrappers duplicate `require_provider_version`, the flock/claim sequence, and `is_dux_worktree`. Audit03 `06-arch:46` declined the extraction |
| A20. SQLite downgrade guard | not filed by any audit | — | **NOT DONE** | P1 | `src/storage.rs:166-178` applies migrations where `version > user_version` with **no guard for `user_version > MAX_MIGRATION`**. An older binary silently opens a newer schema |
| A21. Mouse capture leaks on panic | not filed | — | **NOT DONE** | P2 | `EnableMouseCapture` at `mod.rs:1690`, `DisableMouseCapture` only on normal exit (`:1831`). `ratatui::init()`'s hook restores raw mode and the alt screen, not mouse reporting |
| A22. `dux-config-changes.toml` documented but unused | `dux-amq/README.md:34` | described as the config diff applied post-launch | **DIVERGED** | P3 | `install.sh` never reads it; the real patching is the hand-written `sed -i` block at `dux-amq/install.sh:421-435` |

### Operations, docs, process

| item | source | claimed | verified at HEAD | sev | what remains |
|---|---|---|---|---|---|
| **O1. Doctor has no encryption section — and `SECURITY.md` promises one** | plan `audit02/25:128`; `SECURITY.md:126,135-136` | "doctor only emits a TODO marker" | **NOT DONE, and the deferral's claim is false** | **P1** | `grep -ni encrypt dux-amq/scripts/dux-amq-doctor` → **0 hits**. `grep -n encrypt src/cli.rs` → 0. There is no TODO marker and no section. `SECURITY.md:126` tells operators to `grep -E '(…\|encryption)'` during an incident; they get silence |
| O2. `scripts/validate-threat-model.sh` | `SECURITY.md:157-161` | "planned" | **NOT DONE + unbuildable as specified** | P2 | No `scripts/` dir; the specified input tree was deleted by HEAD |
| O3. T11 symlink-swap mitigation | `SECURITY.md:61`; `threat-model.md:424-435` | status `future` | **NOT DONE** | P2 | "Symlink target check on launch (planned)" |
| O4. Two of three install-idempotency tests never run in CI | `dux-amq/tests/install-idempotency.bats:32-36,143,212` | — | **NOT DONE** | P1 | `require_install_env` skips unless `/data` is mounted **and** `dux`/`amq` are on PATH — none true on the `ubuntu-24.04` runner. Only the hermetic P0-01 test at `:63` executes. The P0-F "second install preserves AMQ config" guarantee is CI-unverified |
| O5. Ubuntu 22.04 kernel matrix | plan `audit02/27:168` | explicitly deferred | **NOT DONE** | P3 | Only 24.04 captured |
| O6. Scheduled TIOCSTI probe | audit01 P1-1; plan `audit01/07:67` | plan unticked | **NOT DONE** | P2 | `tiocsti-detect.bats` (6) exercises `tiocsti_status` against a **fake procfs file** only. No live-kernel probe, no scheduled workflow |
| O7. Upstream AMQ issues filed | plans `audit02/08:222`, `audit02/13:165` | deferred (no PAT) | **presumed NOT DONE** | P3 | Both drafts remain in-tree artifacts |
| O8. bats never runs on macOS | `overlay-ci.yml:21`; `PLAN-REVIEW-LOG.md:329` | "2 known macOS finalize failures" tolerated | **accepted, unverified** | P2 | Nothing in the bash overlay is CI-tested on macOS |
| O9. Audit03 was never dynamically validated | `docs/audits/audit03/00-summary.md:16` | acknowledged | — | P2 | "No build, test, Clippy, audit, deny, or Make target was run" |
| O10. Audit03 ledger unmaintained | `99-disposition.md:5,7` vs rows `:13-51` | — | **STALE** | P2 | Prose still says "39 confirmed… every finding is in state `confirmed`" while all 39 rows say `fixed`. Cited SHA `b13df41` unreachable. **Ten `audit03 …(review …)` commits** (`1da919d`, `101c1db`, `1551996`, `a8a0ae4`, `49f73d2`, `e80151a`, `ec52425`, `98f2b81`, `f2452e8`, `1d8c98d`) are absent from the ledger despite its own instruction at `:7` |
| O11. PLAN.md / RESUME-PLAN.md never closed | `PLAN.md:100`; `PLAN-REVIEW-LOG.md:417` | — | **STALE** | P3 | `PLAN.md:100` lists provider-conversation-ID persistence as out of scope; RESUME-PLAN implements exactly that (migration 0006, `SessionLaunch`, `provider_session_ids`, T18) with no supersession note. `PLAN-REVIEW-LOG.md:417` still says "Nothing merged to main"; main is now at a `shared-workspace:` commit |
| O12. Accepted-risk TOCTOUs | `PLAN-REVIEW-LOG.md:391,308` | "accepted-risk (same-UID)… no fix" | **accepted** | P3 | Guard-canonicalize vs raw-path removal; `assign_unique_agent_handle` read→upsert |
| O13. `CLAUDE.md` is factually wrong in two places | `CLAUDE.md:86-87,164` | — | **STALE** | P2 | Describes `SessionStatus` as "persisted on every row today" with phase 2 pending (both wrong — `grep -rn SessionStatus src/ tests/` yields one SQL comment); lists 5 `SessionState` variants when there are 6; asserts clippy is a merge gate when branch protection says otherwise |

### Done and verified (terse)

**audit03 closed cleanly: 37 of 39 findings fully verified closed** in source at HEAD; S6 (P1-08) and S3/S5
(P1-13) are the two partials. Notable verified closures: P0-02 no-replace renames
(`amq_inject.rs:316,419,614`); P0-03/P0-04 purge containment (`purge.rs:488,530-532,613,714-716`); P0-05
atomic `.git/info/exclude` (`git.rs:383-397,416-430`); P0-06 bounded PTY teardown (`pty.rs:662-724`); P1-15
atomic migrations (`storage.rs:229-244`); P1-28 atomic config write (`config.rs:2187-2239`).

audit01/02 verified done: YOLO opt-in both providers; seeding opt-in; `flock` + `mv -Tn` migration safety;
sha-pinned install chain + hash-guarded `eval`; wake startup fails closed; auto-resume concurrency/staleness
(note the field is `stale_days` defaulting to **30**, not `auto_resume_max_age_days`=14); sanitizer with 152
call sites; PTY poison tolerance + reader join; GDPR purge incl. log redaction (`purge.rs:779`); doctor with
`--json`/`--anonymize`; AMQ HMAC scripts; SQLite WAL + integrity + versioned migrations; `ensure_column`
identifier allowlist; `[limits]` caps; **all 24 GHA `uses:` SHA-pinned**; CODEOWNERS; dependabot.

**Watch Phase 3 did land** (contrary to the `#[allow]` comments): `App::open_watch_rules_prompt`
(`app/mod.rs:3611`) is dispatched from the palette (`"watch-rules"`, `:2292`), renders at `render.rs:5224`,
keys at `input.rs:2522`.

**Two stale deferrals to correct in the docs:** plan `audit02/18:146` says `providers: HashMap` removal is
"blocked on PR #5" — done (`app/state/runtime.rs:25-28`). Plan `audit02/27:172` says the `audit02-validated`
tag was deferred — it is pushed (`3191f1e`).

---

## In-code debt

Hygiene is genuinely good. The debt is concentrated in dead code, suppressed diagnostics, and doc rot — not in
panic-prone or unsafe code.

### Markers

| category | count | detail |
|---|---|---|
| `TODO`/`FIXME`/`HACK`/`XXX`/`unimplemented!`/`todo!` | **0 real** | All 18 grep hits are `mktemp .XXXXXX` templates or the literal fixture `"XXX"` (`diff.rs:633`) |
| `ponytail:` debt markers | **3, all deliberate and well-scoped** | `app/workers.rs:3102` (per-diff SyntaxCache — states cause, cost, fix); `dux-amq/scripts/dux-amq-inject-bridge:89` (blocked on an upstream AMQ feature); `dux-amq/install.sh:329` (temp-root list, documents the `CLAUDE_PEERS_DIR` escape hatch) |
| historical phase / follow-up / dead-code markers | **192** | Audit03 counted 83 at its 2026-07-11 baseline (`06-arch:50`) — more than doubled in seven weeks. `06-arch:50` declined to clean them |
| `#[ignore]`d tests | **0** | Clean |
| `unsafe` in production | **1** | `src/pty.rs:649` `BorrowedFd::borrow_raw`. **Exemplary**: model SAFETY comment, `debug_assert!` guard, field-level "DO NOT remove" doc at `:77-83`, compensating `let _ = &self.master;` in `Drop`. All other `unsafe` hits are `env::set_var` under `#[cfg(test)]` |
| `#![forbid(unsafe_code)]` / `[lints]` / `clippy.toml` / `.cargo/config.toml` | **none** | The single audited `unsafe` block is protected by review convention only; clippy config lives entirely in the CI command line |

### `#[allow]` scorecard — 41 total

By kind: `dead_code` 28 (27 item-level + 1 file-level) · `deprecated` 7 · `clippy::too_many_arguments` 2 ·
`new_without_default` 1 · `should_implement_trait` 1 · `enum_variant_names` 1 · `unused_imports` 1.

**29 carry a rationale; 12 carry none. Of the 29 justified, 6 justifications are factually false** (verified
against the compiler).

Undocumented (12): `src/theme.rs:1` (file-wide `#![allow(dead_code)]`), `src/config.rs:1261`,
`src/amq_inject.rs:176,341`, `src/pty.rs:296,378,760`, `src/app/mod.rs:1069`, `src/app/workers.rs:3088`,
`src/app/components/checkbox.rs:8`, and six test-side `#[allow(deprecated)]` at `src/storage.rs:1213-1266`.

Justified but false: `app/text_input.rs:286` ("exercised by tests" — dead even under `--all-targets`),
`amq_inject.rs:593` ("used by tests" — dead), `purge_encoding.rs:51` ("future call sites" — dead),
`watch/engine.rs:246,259` ("Phase 3 wires this up" — `disarm`/`rearm` **are** used at `app/mod.rs:3669,3412`
and `app/sessions.rs:2772`, so the allows are now dead weight), `watch/engine.rs:236` (tests only, not Phase 3).
`src/pty.rs:760` is stale — `client()` *is* used; only `client_mut` (`:767`) is dead.

`src/storage.rs:194` is the model to copy: `:190-193` states what `ensure_column` is, why it is deprecated, why
the legacy upgrade path keeps it, and that new schema work must use a numbered migration.

### Dead code — measured, not estimated

Neutralizing the 28 `dead_code`/`unused_imports` allows and running `cargo check --all-targets` yields **38
distinct dead items** (identical under `--lib --bins`, i.e. tests rescue none of them). Highlights:

- `src/keybindings.rs:1659` — the `show_terminal_keys` dead-config bug (A8 above).
- `src/model.rs:142,224` — the dead `SessionState` typestate API (A1 above), plus `app/mod.rs:3064`'s ~80 lines
  of dead detach/reattach machinery.
- `src/config.rs:1266` — `DeprecatedConfigKeyAction::{Remove, Fail}`: a config-migration taxonomy declared and
  never used.
- `src/watch/mod.rs:27` — 5 unused re-exports.
- `src/amq_inject.rs:177,342,594` — `scan_queue_dir`, `reclaim_stale_inflight`, `release`: superseded by
  `_limited`/`_with_max_age` variants. Should be deleted, not allowed.
- `src/pty.rs:379,768,905` — `snapshot`, `client_mut`, and `TerminalState::snapshot` (dead only because of the
  first — cascading).
- `src/theme.rs:80,544,577` — the file-wide allow hides exactly **3** items: `hint_key_bg`, `default_dark`,
  `status_style`. Suppressing a 976-line module's diagnostics for three constants.
- `src/app/components/checkbox.rs:11` — `Hovered`, `Disabled`: a widget with states it cannot enter.
- `src/app/mod.rs:627,1254,1281` — speculative fields (`provider`, `remote_default`, `worktree`+`message`).

**Build-level debt:** `src/lib.rs` re-declares every module `src/main.rs` declares, so the whole ~69.5k LOC
compiles **twice** per build — confirmed by the two `cargo check` runs emitting independent warning sets for
the same source lines. `src/lib.rs:8-9` acknowledges it; nothing mitigates it. `main.rs` could be a ~10-line
shim over the lib. This roughly doubles build and clippy time, and `crash.rs` is bin-only so integration tests
never exercise the panic hook.

### Panic and crash vectors

**33 `unwrap()`/`expect()` in production code** (before each file's first `#[cfg(test)]`), out of 1,467 total
in `src/`. Excellent ratio. Ranked risks:

1. **`impl Drop for CodexCapture` panics on mutex poison — `src/resume_recovery.rs:343`.** A poisoned mutex
   here means a *second* panic during unwinding → `abort()`, which bypasses `crash::install_panic_hook`
   (`crash.rs:9`) **and** ratatui's terminal-restore hook. Result: terminal left in raw mode + alternate screen
   with no crash log. Reachable on the UI thread — `FreshCapture::Codex(CodexCapture)` is moved into
   `WorkerEvent::AutoResumeSpawnOnMain` (`app/mod.rs:1299,1943`) and dropped in `drain_events`
   (`app/workers.rs:738`). **Highest-severity in-code defect found.**
2. **`impl Drop for Permit` — `src/auto_resume.rs:57`.** Same abort shape, on the auto-resume worker thread.
3. `src/resume_recovery.rs:150,158,277,354` — same `.expect` on poison, not in `Drop`.
4. **`src/storage.rs:446`** `conn.lock().expect("storage mutex poisoned")` — the single funnel for every DB
   access, called from the UI thread. One panic in any storage caller poisons it and turns every subsequent
   frame into a panic. Documented at `:443-445`, but the fix is `unwrap_or_else(|e| e.into_inner())`.
5. `src/clipboard.rs:36` — `.expect` on `thread::spawn` (the unfixed audit01 P2-2).
6. Mouse-capture leak on panic (A21).
7. `src/sanitize.rs:52` — `.take(max_chars - 1)` with no `max_chars == 0` guard in a `pub` general-purpose
   helper. Unreachable today (callers pass 300/512/60/80), but `[profile.release]` sets no `overflow-checks`,
   so in release it wraps to `usize::MAX` and silently stops truncating.
8. `src/storage.rs:361` — raw `AGENT_HANDLE_MAX_LEN - suffix.len()` where `src/peer.rs:1027` does the identical
   computation with `saturating_sub`. Inconsistent; needs ~10^57 collisions to fire.
9. `binding.palette_name.unwrap()` ×4 (`render.rs:2291,2299`, `input.rs:1929,4328`) — safe because
   `filtered_palette` (`keybindings.rs:1832`) only yields `Some`, but the invariant is unencoded; a
   `filter_map` returning `(&RuntimeBinding, &str)` would eliminate all four.

`src/pty.rs` handles mutex poisoning gracefully at **every** site (`let Ok(…) else`, `.ok()?`, `is_ok_and`) —
that is the model the four `resume_recovery`/`auto_resume` sites should copy.

`std::process::exit` in library code: `peer.rs:283`, `cli.rs:235,251,289,308`, `app/mod.rs:1481`,
`main.rs:166`. `app/mod.rs:1481` exits during bootstrap before the PTY map is populated, and the lockfile has
stale-PID recovery (`lockfile.rs:293`), so this is tolerable rather than correct. The `unreachable!` at
`lockfile.rs:235` is inside `#[cfg(test)]`; `storage.rs:367` and `peer.rs:1034` are genuinely unreachable.

### UTF-8 and integer safety — clean

`CLAUDE.md:142` is genuinely respected. Every truncation helper is char- or width-based: `sanitize.rs:45`,
`app/render.rs:7156` (per-char `Line::width()`, grapheme-width aware), `cli.rs:527`, `watch/engine.rs:466`,
`git.rs` `ellipsize_middle`. Every byte-index slice traced to a real char boundary (`render.rs:6938`
`split_at(cursor)` with `cursor` maintained by `prev/next_char_boundary`; `render.rs:6341` from
`char_indices().nth()`; `purge.rs:906` guarded by `ends_with('\n')`; `watch/reset_time.rs:106` from `find()`).
There is a dedicated regression test at `render.rs:7552`
(`truncate_status_text_handles_cjk_emoji_and_combining_boundaries`).

Scroll/layout arithmetic is likewise guarded — every raw subtraction traced has a real guard
(`render.rs:264,1029-1030,1069,1250-1251,1368,2425`; `input.rs:4265,57,62`; `pty.rs:1019,1345`;
`text_input.rs:608`; `diff.rs:432`), and `saturating_sub` is used consistently. The only inconsistency is
`storage.rs:361` vs `peer.rs:1027`.

### Logging

113 `tracing::` call sites, **0 missing `target:`**, **0 `tracing::trace!`** (correctly, since the env-filter
default drops it). 50 legacy shim sites remain (A16). Sanitization has no exploitable gap, but
`amq_inject::preview` only maps `\n`/`\r` → space and does **not** strip OSC/CSI — it is safe solely because
`read_validated` rejects bodies containing control chars other than `\n`/`\t`, an invariant enforced ~500 lines
away in a different module with neither site documenting the coupling.

Coverage is thin and lopsided: 113 statements across 69.5k LOC, concentrated in four files (`inject_runtime`
28, `workers` 22, `mod` 14, `logger` 13). `src/app/render.rs`, `src/app/input.rs`, `src/keybindings.rs`,
`src/git.rs`, and `src/diff.rs` emit **zero** structured events — while `CLAUDE.md`'s debugging playbook tells
operators to "first check `dux.log`".

### Bash — the healthiest part of the repo

`set -euo pipefail` in **12/12** files. Only **3** `# shellcheck disable=` directives, all benign (SC2016 in
the doctor; SC1090/SC1091 in two bats files) — none masks an unquoted expansion. 37 `|| true` occurrences, all
but one the idiomatic `x=$(cmd 2>/dev/null || true)` capture form; the exception,
`seed_session_history || true` (`dux-amq/wrappers/claude-amq:161`), is deliberately best-effort and the block
at `:128-134` documents the fixed audit02 P1-D bug where rsync errors *were* swallowed. `Makefile:26`
shellchecks every executable under `dux-amq` plus `install.sh`, and CI runs it on every PR. The only
bash-adjacent finding is the SECURITY.md ↔ doctor drift (O1).

---

## Coverage map

`cargo test` = 1,139 distinct tests. `tests/` holds 12 tracked integration files. bats holds 123 across 14
suites, ubuntu-only.

| subsystem | source (LOC) | tests | verdict | what it actually pins |
|---|---|---|---|---|
| Key/mouse input | `app/input.rs` (13,266) | 236 in-file | **good** | Real `App` fixture (`:6238`) with real SQLite, real lockfile, real PTYs, real `git init`. 79 `handle_key`, 111 `handle_mouse`, 29 `process_raw_input_bytes`. Holds the only 15 `TestBackend` draws in the repo (`:8893`…`:12876`) |
| Rendering | `app/render.rs` (7,565) | 43 in-file | **thin** | Pure helpers only. **Zero tests call any `render_*` method.** 21.28% measured |
| App core | `app/mod.rs` (4,447) | 13 | **thin** | 42.97%. `App::run` (`:1671`) untested by construction (`ratatui::init()`); `install_pty_for_session` (`:3100`) — the PTY-ownership transfer — untested |
| Background workers | `app/workers.rs` (4,394) | 16 | **thin — worst blast radius** | 20.99%. `drain_events` (`:8`, ~1,050 lines, 40 `WorkerEvent` variants) sees ~5 variants; ~35 handlers never execute. One test (`:4384`) is a source-text grep of its own file via `include_str!` — proves nothing and breaks on rename |
| Session CRUD | `app/sessions.rs` (4,713) | 53 | **good** | Second full `App` fixture (`:2848`). Shared-writer confirmation, detach conflicts, tombstones, per-provider resume argv, pane/disk/scrollback limits |
| Config | `config.rs` (4,952) | 95 | **good** | 91.77%. Canonical render → parse → round-trip per section; `toml_edit` comment preservation; all 4 migration classes; `expand_path` traversal rejection; atomic-replace failure keeps prior bytes |
| Keybindings | `keybindings.rs` (2,862) | 59 | **good — highest refactor leverage** | 93.02%. Structural invariants: `every_action_has_config_name`, `every_keyed_binding_has_help_entry`, `every_help_entry_section_is_rendered`, `new_actions_are_in_binding_defs`, `detect_conflicts_default_config_clean`. Moving an action without wiring config-name + help entry fails immediately |
| PTY | `pty.rs` (2,161) | 33 + 9 integration | **good** | 78.67%. Real PTYs. Scrollback round-trip, offset stability under concurrent output, alt-screen/mouse-mode detection from real escapes, drop-promptness incl. background descendants |
| Peer router | `peer.rs` (2,281) | 32 | **good** | Transport policy matrix, cwd disambiguation, handle collision suffixing, `subprocess_helper` re-exec for real concurrency, and `rust_and_wrapper_claims_serialize_on_config_lock` — **the Rust↔bash flock interop seam** |
| Git | `git.rs` (2,236) | 58 + 2 integration | **good** | 84.82%. `--porcelain=v1 -z` / `--numstat -z` parsers incl. renames and binaries; worktree-removal branch ownership; containment incl. symlink aliases; non-UTF-8 paths; bounded process count under untracked growth |
| Storage / migrations | `storage.rs` (1,485) | 15 + 26 integration | **good** | WAL, integrity-check → `Err` not panic, backup validity, `SessionSettings` NULL/malformed fallback, fail-closed on duplicate handle. Migrations 0002–0006 each pinned for `user_version` bump, idempotency, 0005 backfill + FK preservation + atomic rollback |
| Session lifecycle | `model.rs` (887, 0 in-file) | `tests/session_state.rs` (7) | **good, but tests dead code** | 83.78%. Pins illegal/self transitions, `Spawning`→`Retryable`, `Detached`→`Created`. **The API it tests is never called by the binary (A1)** — the tests are the only consumer |
| Watch engine | `watch/*` (1,777) | 40 + 5 integration | **good** | Integration test drives `cat` in a real PTY: regex match → Idle→Pending→fire, backoff/cooldown timing, `SendText` round-trip |
| AMQ inject | `amq_inject.rs` + `app/inject_runtime.rs` | 69 + 4 integration | **good** | Real queue dir + real PTY. scan/claim/`read_validated`/`reclaim_stale_inflight`; bracketed-paste per provider; receiver-match priority; quiet-window boundaries; `[task-done]` postscript |
| GDPR purge | `purge.rs` (1,276) | 7 + 22 integration | **good** | 69.05%. Full fake install; dry-run no-op; wrong-confirmation abort; documented execution order; log redaction scoping; never frees a foreign inbox; per-category failure retains the row |
| Resume recovery | `resume_recovery.rs` (1,260) | 10 | **thin for its size** | Atomic no-clobber copy, symlink refusal, per-cwd serialization, bridge-stub rejection. Encoding pinned by one hardcoded string, not the shared fixture |
| Text input / raw input | `app/text_input.rs` (1,804) / `raw_input.rs` (763) | 93 / 53 | **good** | 95.30% / 96.73% |
| CLI / main | `cli.rs` (1,991) / `main.rs` (169) | 19 / 0 | **thin / none** | 52.97% / **0.00%** |
| App sub-state | `app/state/*` (297) | 0 | **none, correctly** | Pure field bags with no behaviour (A3) |
| Bash overlay | `dux-amq/` (3,627) | 123 bats | **good for scope, ubuntu-only, 2 tests skip in CI** | See below |

### The two headline gaps

**TUI render snapshot testing does not exist.** `TestBackend` *is* used — 15 call sites, all in
`src/app/input.rs` — but every one flattens the buffer to a `String` and asserts `rendered.contains("…")`
(e.g. `:8918` `contains("DIRTY")`, `:12844` `contains("Codex agent")`). No `insta`, no `.snap`, no
`expect-test`; dev-deps are only `filetime` + `serial_test`. Nothing pins layout — pane widths, borders,
ordering, styles, colours, truncation points, widget position. `render.rs` is 7,565 lines at 21.28% whose 43
tests touch only leaf helpers. **Split `render.rs`, or move a field a renderer reads onto a sub-struct, and the
whole suite stays green while the UI visibly regresses.** For a decomposition project this is the single most
important missing artefact.

**Coverage tooling is not configured.** No `llvm-cov`, `tarpaulin`, `grcov`, or codecov in `.github/`,
`Makefile`, or `Cargo.toml`. The last measured number — **64.35% regions / 64.65% lines**
(`git show HEAD^:docs/plans/audits/audit02/artifacts/27-coverage.txt`) — is from **2026-05-04** and describes a
smaller program: it predates `peer.rs`, `resume_recovery.rs`, `orphan_worktrees.rs`, `orchestrator.rs`,
`amq_activity.rs`, `amq_inject.rs`, `inject_runtime.rs`, and all of `watch/` — ~11,000 of today's 69,515 lines.
Treat 64% as an upper bound on a different codebase. During a refactor, coverage can only fall silently.

No proptest, quickcheck, or fuzzing.

### Refactor-specific hazards

- **Two hand-built `App` fixtures** (`input.rs:6238`, `sessions.rs:2848`) each construct `UiState` +
  `RuntimeState` + `GitState` + ~40 `App` fields with **no `..Default::default()`**. Any field added, moved, or
  removed forces edits in both. Extracting one shared `#[cfg(test)] AppBuilder` is step zero.
- Both call `std::mem::forget(tmp)` (`input.rs:6241`, `sessions.rs:2850`). With 198 + 49 fixture calls, one
  `cargo test` run **leaks 247 tempdirs** containing SQLite DBs and lockfiles into `/tmp`, permanently.
- Real work in the suite: 52 `PtyClient::spawn`, 69 `Command::new("git")`, 57 `thread::sleep` sites. Timing
  tests are honestly written (`tests/auto_resume.rs` allows 50% slack; `drain_until` at `input.rs:6443` polls
  200×10ms). No network anywhere.
- `tests/git_portability.rs` is `#![cfg(target_os = "linux")]` — never runs on a macOS dev machine.
- `serial_test` used exactly 3× (`config.rs:4882,4895,4925`, guarding env mutation) — appropriate.
- An untracked scratch test `tests/zz_tmp_sizeof.rs` was present at the start of this research and has since
  been removed by a concurrent agent; it is not in `git ls-files`. Mentioned only so a stale reference elsewhere
  does not confuse.

### Bash behaviour pinned only by bats

The 123 bats tests are the **sole executable specification** for: the DUX2 HMAC envelope end-to-end
(`amq-auth.bats`, 13 — field regexes, receiver binding, concurrent replay rejection, and the poison-exits-0 vs
setup-exits-1 asymmetry that stops AMQ retrying poison); wrapper ownership + mandatory fail-closed `flock`
(`ownership.bats` 7, `wrappers-p1.bats` 20 — incl. handle validated *verbatim*, so `Bad/Handle` is rejected
rather than repaired); Claude project-dir encoding (`encoder-fixtures.bats` 8, fixture-shared with Rust);
`is_dux_worktree` containment incl. the `worktrees-evil/` sibling-prefix attack and the assertion that the
helper is byte-identical across all three wrappers; install idempotency and the binary-integrity `eval` guard
(`install-idempotency.bats` 3 — **only 1 runs in CI**, `root-installer.bats` 2, `supply-chain-rails.bats` 4);
the TIOCSTI rc contract — absent→2, `1`→0, `0`→1, garbage→2, empty→2, i.e. unknown states fail toward
"compiled-out" (`tiocsti-detect.bats` 6); inject-bridge routing incl. `.unrouted`, dead-`DUX_PID` drop, and
sanitised-`AM_ME` queue keys (`inject-bridge.bats` 22); finalize-migration atomicity (5); legacy CLAUDE.md
block stripping (3); the doctor's read-only guarantee and `--anonymize` redaction (7).

Caveats: the suite drives **fakes** (`dux-amq/tests/fakes/{amq,claude,codex,gemini,flock}`); the `flock` fake
is a macOS shim falling back to Ruby `File#flock` then `mkdir`-spinning, so concurrency tests prove wrapper
logic under *a* lock, not kernel `flock` semantics.

---

## CI & supply chain

### Gates

| gate | file:line | blocking on merge? | platform |
|---|---|---|---|
| `cargo fmt --check` | `pr.yml:30` | **No** (F4) | ubuntu-24.04 |
| `cargo clippy --all-targets --all-features -- -D warnings` | `pr.yml:63` | **No** (F4) | ubuntu-24.04 + macos-14 |
| `cargo test --all-features` | `pr.yml:88`, `test.yml:40` | doc says yes; artifact says no required checks were set | ubuntu-24.04 + macos-14 |
| `cargo audit --deny warnings` (2 `--ignore`) | `pr.yml:128`, `test.yml:73` | as `Security` | ubuntu-24.04 |
| `cargo deny --all-features check` | `pr.yml:133`, `test.yml:78` | as `Security` | ubuntu-24.04 |
| `shellcheck` via `make overlay-shellcheck` | `overlay-ci.yml:35` | as `shell` | ubuntu-24.04 |
| `bats dux-amq/tests` | `overlay-ci.yml:38` | as `shell` | ubuntu-24.04 only |

No MSRV job, no `cargo semver-checks`, no `cargo vet`/`crev`, no coverage job. `--locked` only on tooling
installs (S7). **Action pinning is complete and correct** — all 24 `uses:` lines pinned to 40-char SHAs with
version comments. `dependabot.yml` covers cargo + github-actions weekly.

`deny.toml` is well constructed: `yanked = "deny"`, `wildcards = "deny"`, `multiple-versions = "warn"` with one
justified `skip` (signal-hook), a strict SPDX allowlist with per-entry rationale, crates.io-only sources with
`allow-git = []`. Its weaknesses are S1–S5 above.

### Release pipeline

Mature for what it builds: four targets (`x86_64/aarch64-unknown-linux-musl`, `x86_64-apple-darwin` on
`macos-13`, `aarch64-apple-darwin` on `macos-14`, all pinned OS versions), `cargo auditable build` (`:116`),
`strip` (`:119`), CycloneDX SBOM (`:130`), double-package + `cmp` (`:136-147`),
`actions/attest-build-provenance` (`:154-158`), and a `SHA256SUMS` job (`:186-218`). Gaps: S6, S7, S8, S9, S16,
S15, plus no Homebrew tap of this fork, no `cargo publish`, no packaging manifests — and per F1 the pipeline has
only ever run on upstream-lineage tags.

### Dependency currency

Verified against `crates.io/api/v1/crates/<name>` on 2026-08-31. 373 crates in `Cargo.lock`.

| crate | Cargo.toml | Cargo.lock | latest | status |
|---|---|---|---|---|
| **rand** | `0.8` | **0.8.6 + 0.10.1** | 0.10.2 | **TWO majors behind**; 0.10 already in the tree (via petname) — the bump also removes the duplicate **and the yanked `chacha20` (S2)** |
| **notify** | `7` | 7.0.0 | **8.2.0** | **One major behind — and 8.x is the fix for RUSTSEC-2024-0384 (S3)** |
| **sysinfo** | `0.35` | 0.35.2 | **0.39.6** | Four breaking 0.x minors |
| **rusqlite** | `0.39.0` | 0.39.0 | 0.40.2 | MAJOR-behind (0.x minor = breaking); bundled SQLite moves with it |
| **compact_str** | `0.9` | 0.9.0 | 0.10.0 | MAJOR-behind (0.x) |
| **serial_test** (dev) | `3.5.0` | 3.5.0 | 4.0.1 | MAJOR-behind |
| **content_inspector** | `0.2.4` | 0.2.4 | 0.2.4 | **Effectively abandoned — last release and last repo push 2018-11-04 (7.8 years)**. It is a BOM/binary sniffer; ~30 lines inline would remove a frozen dep from the SBOM |
| **ratatui** | `0.30.0` | 0.30.0 | 0.30.2 | Patch behind — **and this is what pulls the vulnerable `lru` (S1)** |
| **petname** | `3.0.0` | 3.0.0 | 3.2.0 | Minor behind — **pulls `rand 0.10.1` → yanked `chacha20 0.10.0` (S2)** |
| **opaline** | `0.4` | 0.4.0 | 0.4.1 | Patch behind. See risk note below |
| crokey | `1.4.0` | 1.4.0 | 1.5.0 | Minor behind |
| similar | `3` | 3.1.0 | 3.2.0 | Minor behind |
| regex | `1` | 1.12.3 | 1.13.1 | Minor behind |
| uuid | `1.17.0` | 1.23.0 | 1.26.0 | Minor behind |
| toml / toml_edit | `1.1.2` / `0.25` | 1.1.2 (+0.8.23) / 0.25.11 (+0.22.27) | 1.1.4 / 0.25.13 | Patch behind; **duplicate majors in tree, pulled in by `opaline`'s `toml ^0.8`** and invisible in CI because `multiple-versions = "warn"` |
| anyhow / chrono / serde / serde_json / indexmap / filetime | — | 1.0.103 / 0.4.44 / 1.0.228 / 1.0.149 / 2.14.0 / 0.2.27 | 1.0.104 / 0.4.45 / 1.0.229 / 1.0.151 / 2.14.1 / 0.2.29 | Patch behind |
| signal-hook | `0.4` | 0.4.4 (+0.3.18) | 0.4.4 | Direct dep current; 0.3.18 via crossterm/termwiz — the `deny.toml` skip is warranted |
| portable-pty | `0.9` | 0.9.0 | 0.9.0 | Current, but upstream last released **2025-02-11** (18 months); low cadence |
| alacritty_terminal / crossterm / syntect / arboard / home / rustix / tempfile / tracing* | — | 0.26.0 / 0.29.0 / 5.3.0 / 3.6.1 / 0.5.12 / 1.1.4 / 3.27.0 / 0.1.44,0.3.23,0.2.5 | same | Current |

**Must-upgrade, in order:**
1. **`cargo update` (lockfile only, no API churn)** — clears S1 and S2, and picks up ~12 patch/minor bumps.
2. **`notify` 7 → 8** — the only way to drop RUSTSEC-2024-0384 and one of the two standing ignores.
3. **`rand` 0.8 → 0.10** — two majors; `small_rng` still exists; collapses the duplicate.
4. **`sysinfo` 0.35 → 0.39**, **`rusqlite` 0.39 → 0.40**, **`compact_str` 0.9 → 0.10**, **`serial_test` 3.5 → 4.0**.
5. **Reassess `content_inspector`.**

**`opaline 0.4` risk assessment.** Publisher Stefanie Jane / hyperb1iss, `github.com/hyperb1iss/opaline`, MIT,
88 stars, actively developed (last push 2026-05-20). First published **2026-02-25** — six months old.
**16,073 total downloads.** Five releases in three months. It is a single-maintainer, effectively
zero-adoption-outside-this-project crate compiled into the shipped binary. crates.io publishes are unsigned, so
the mitigation is human review — and neither `cargo vet` nor `cargo crev` is configured (S13). This is the
weakest link in an otherwise well-hardened supply chain and it appears nowhere in `SECURITY.md`. It also drags
the duplicate `toml 0.8` / `toml_edit 0.22` into the tree.

**Toolchain.** `rust-toolchain.toml:12` pins **1.88.0** (2025-06-23); current stable is **1.98.0**
(2026-08-18) — **ten releases, ~14 months**. The pin is hardcoded a second time in every workflow
(`pr.yml:26,49,80,107`; `test.yml:29,55`; `release.yml:79`), so a bump is a 7-line diff. It does **not**
currently block the required upgrades (`cargo update --dry-run` resolves cleanly under 1.88). But every clippy
lint added between 1.89 and 1.98 is invisible, which means the green clippy baseline is weaker than it looks —
and since clippy is `-D warnings`, the eventual bump produces one large cleanup diff. The rustport baseline
(`docs/plans/rustport/artifacts/00-baseline.md:28-32`) already names this a hard prerequisite: the bump must
land **before** the decomposition, "otherwise the refactor ships against a stale lint set and the bump later
produces a second, larger cleanup."

---

## Security posture

The threat model is unusually strong for a project this size: `SECURITY.md` carries a 19-row STRIDE table with
a ~840-line long-form companion. Accepted risks are named and argued rather than hand-waved. Tally: 13
mitigated, 2 accepted-risk (T2, T14-by-inheritance), 2 planned-only (T10, T11), 1 documentation-only (T5), T3
partial.

Spot-verified: **T13's claims hold in code** — `REGEX_SIZE_LIMIT = 64*1024` (`src/watch/engine.rs:38`),
`REGEX_DFA_SIZE_LIMIT` (`:40`), `MAX_RULES_PER_PROVIDER = 32` (`:43`), applied at `:155-157`; `cooldown_ms`
default 30,000 (`src/watch/rule.rs:44`), `budget.max_attempts` default 5 (`:208`), `0 == unlimited` (`:214`).

**T3 is partial and the table overstates it.** `dux-amq/install.sh:283-305` pins the verified binary at
`$STATE_ROOT/amq-bin/amq` and `bashrc-additions.sh:25,63-64` refuses to `eval` unless the hash matches. But
**`AMQ_BIN` and `amq-bin` appear nowhere in `src/`** — the Rust binary shells out to bare `amq` resolved from
`PATH` at `src/peer.rs:480` and `src/purge.rs:1013`. A tampered `amq` earlier in `PATH` is executed by dux with
no hash check.

### Attack surface with no threat ID

1. **The Claude Peers localhost broker — `src/peer.rs:548-590`.** `TcpStream::connect_timeout` to
   `127.0.0.1:$CLAUDE_PEERS_PORT` (`:553-554`) speaking hand-rolled HTTP/1.1 (`:560-570`). Grepping
   `SECURITY.md` and all 840 lines of `threat-model.md` for `claude peers` / `127.0.0.1` / `loopback` /
   `localhost` / `tcp` returns **zero hits**. Specific gaps: (a) **no authentication** — no token, no HMAC, no
   nonce, in direct contrast to T2's envelope for the AMQ path; any local process reaching that port can post;
   (b) **`CLAUDE_PEERS_PORT` is an unvalidated env override** (`:549-552`), so control of dux's environment
   redirects a channel carrying repo-derived message bodies to an arbitrary local sink; (c)
   **`read_to_string` at `:573` is unbounded** — a hostile or wedged broker returns an arbitrarily large body
   and OOMs dux; the 3-second read timeout at `:556` bounds time, not size. `CLAUDE.md` mandates a
   `SECURITY.md` update in the same PR for "a new network egress"; this is the only network egress in the
   binary and it has none.
2. **The Claude Peers MCP server install — `dux-amq/install.sh:337-372`** (S11). Third-party TypeScript
   registered as a user-scoped MCP server for every Claude session on the box, with `bun install` running
   unpinned and scripts-enabled. `SECURITY.md:15-28` "Scope" does not mention it; `CLAUDE.md` explicitly names
   "a new MCP integration" as requiring a STRIDE row.
3. **`src/crash.rs:24-28,41-45` — unsanitized, unbounded, world-readable crash log.** Panic payload and full
   `Backtrace` written verbatim to `dux-crash.log` with **no `crate::sanitize::for_terminal` call**, unlike
   `dux.log` (T8). Panic messages routinely interpolate attacker-influenced strings (branch names, PTY text),
   so raw OSC/CSI lands in a file operators `cat`. `OpenOptions::new().create(true).append(true)` sets no mode
   → 0644 under default umask, while `sessions.sqlite3` is deliberately 0600 (`src/storage.rs:35,55`). No
   rotation, no size cap, versus `dux.log`'s daily rotation with `max_log_files(7)` (`src/logger.rs:65-69`).
4. **`dux.log` file mode.** `tracing_appender`'s rolling builder sets no mode → 0644. On a shared host the
   JSON-Lines log (worktree paths, branch names, PR titles, sanitized PTY excerpts) is world-readable.
5. **Arbitrary configured provider execution — `src/pty.rs:206-219`, `src/provider.rs:54-62`.**
   `CommandBuilder::new(command)` / `Command::new(&self.config.command)` from `[providers.*].command`. There is
   **no `env_clear()`** on either path (`pty.rs:210-214` only *adds* vars), so children inherit dux's full
   environment including any API keys. `provider.rs:56-62` substitutes `{prompt}` into argv, and the
   commit-message prompt embeds repo diff content, so repo-controlled text can produce an argv element
   beginning with `-` (option injection into the provider CLI; not shell injection). T1 addresses what the
   agent does once running; nothing addresses what dux is configured to run.
6. **`src/purge.rs:1013-1015` — argv option injection.**
   `Command::new("amq").args(["send", branch, "--label", "purge", &body])`. `branch` sits in argv position 2
   with no leading-`-` check. `src/git.rs:1114` rejects `-`-leading names for branches *dux creates*, but a
   pre-existing branch in a cloned repo is not covered on this path.
7. **`src/git.rs` — 44 `Command::new("git")` sites.** Argv arrays throughout (no shell), `--` separators on the
   sensitive paths, correct plumbing/porcelain discipline. But **no `GIT_*` environment hygiene anywhere** —
   every git child inherits `GIT_DIR`, `GIT_CONFIG_GLOBAL`, `GIT_SSH_COMMAND` from dux's environment. Also
   `git pull --ff-only` (`:148`) and the `gh` CLI paths are network egress not enumerated in the threat model.
8. **`npx skills`** (S12) — writes into `~/.claude/skills/`, which `SECURITY.md:21-22` puts *in scope*, with no
   threat ID.
9. **`src/clipboard.rs:122-147`** — OSC 52 written directly to `/dev/tty`, i.e. into the operator's *real*
   terminal outside dux's emulator. Payload is base64 (`:125`) so control bytes cannot escape and there is a
   100 KiB cap (`:10,135`) — reasonable, but an unlisted channel from agent-controlled pane text to the host
   clipboard.
10. **`src/editor.rs:109-120`** — `Command::new(&editor.command)` constrained to a five-name allowlist
    (`EDITOR_SPECS:33-62`) resolved by scanning PATH; the surface is PATH-shadowing, not arbitrary execution.
    Low, but unlisted.
11. **`src/lockfile.rs`** — zero mentions of `lockfile` / `dux.lock` / "single-instance" in `SECURITY.md` or
    `threat-model.md`. `:123` opens with no explicit mode.

`src/orphan_worktrees.rs` (T17), `src/resume_recovery.rs` (T18), and `src/storage.rs` (T4/T15, correct 0600
permissions) are adequately covered.

### Telemetry — definitive verdict

**dux does not phone home, ever, and does not auto-update.**

- Grepping all 373 `[[package]]` entries in `Cargo.lock` for
  `reqwest|hyper|ureq|rustls|native-tls|openssl|curl|isahc|surf|attohttpc|tokio|h2|socket2` → **zero matches**.
  No HTTP client, no TLS, no async runtime in the binary.
- Exactly one socket in the crate: `src/peer.rs:8,554`, destination hardcoded
  `SocketAddr::from(([127,0,0,1], port))` at `:553`. No `TcpListener`, `UdpSocket`, `UnixStream`, or
  `UnixListener` anywhere in `src/`. Loopback-only by construction; only the port is configurable.
- Every `https://` string in `src/` is inert: `config.rs:2704,2785,2823,2848` are `install_hint` display
  strings; `git.rs:1171,1181` are a doc comment and a URL *parser*; `git.rs:2094-2119` are test fixtures;
  `render.rs:2198` is a status message.
- `src/crash.rs:41` writes `root.join("dux-crash.log")` and nothing else. `src/logger.rs:65-80` writes a
  daily-rotated local file. No upload, no symbolication service, no version check, no self-replace.
- The only outbound network capability is *indirect*, via subprocesses dux spawns: `git pull`/`fetch`, the `gh`
  CLI, and the provider CLIs — the user's own tools doing the user's own work.

This is a genuine, defensible telemetry-free claim and `SECURITY.md` should state it explicitly; today it does
not.

### Installer blast radius

`dux-amq/install.sh` (565 lines) requires **no root** (never calls `sudo`; the root path is only in
`root-installer.bats`) and is idempotent by construction. Preflight (`:148-182`) fails closed on a missing
`$STATE_ROOT` parent and aggregates all 11 missing tools into one error.

Inside `$STATE_ROOT`: the eight state dirs (`:183`), the `.tiocsti-state` sentinel (`:196-216`), `amq init`
gated on `meta/config.json` (`:261-267`) + `chmod 700` (`:268`), the pinned `amq-bin/amq` (`:283-299`), a 0444
`binary.sha256` (`:304-305`), install/skills logs, `scripts/finalize-claude-migration.sh` (`:383`),
`dux/config.toml` (regenerated only if absent, `:417-419`, then `sed -i --follow-symlinks` patched
`:421-435`), a timestamped CLAUDE.md backup (`:458-461`), and the claude-peers clone.

**Outside `$STATE_ROOT` — the real blast radius:** ten executables into `~/.local/bin/` plus a `bun` symlink
(`:227,249,340,376-408`); `~/.bashrc` rewritten via versioned markers, with the appended block running
`eval "$("$AMQ_BIN" shell-setup)"` on **every interactive shell** (`bashrc-additions.sh:64`, hash-guarded);
`~/.claude/CLAUDE.md` — a **user-owned document programmatically rewritten** (`:452-467`; the `md` `strip_block`
branch at `:126-142` carries a P0-G fix for a prior bug that deleted everything after the stanza to EOF);
`~/.vscode-server/data/Machine/settings.json` jq-merged (`:488-539`, losing file mode);
`~/.claude.json` mutated via `claude mcp remove`/`add --scope user` (`:365-367`); `~/.claude/skills/` written
by `npx skills add -g` (`:315-317`); and `~/.local/state/claude-peers-mcp` as a temp-root fallback (`:211`).

### What the Rust port changes about the threat model

- The **binary-integrity `eval` guard** (`bashrc-additions.sh:26-30`) is T3's mitigation. Moving it into Rust
  weakens the property from "bash refuses to eval" to "a binary checks itself". Keep the shell-side guard —
  and fix the existing T3 bypass (dux calling bare `amq` from PATH) either way.
- The **DUX2 HMAC envelope** (T2/T14's opt-in rail) exists only in `amq-send-signed`/`amq-receive-verify`. A
  port must reimplement and re-prove constant-time MAC comparison, the atomic nonce-dir replay guard, and
  receiver binding.
- **Cross-language `flock` on `meta/config.lock`** (T7) is load-bearing: Rust and bash must lock the *same
  inode*. `peer.rs`'s `rust_and_wrapper_claims_serialize_on_config_lock` is the only test proving it. A port
  that changes the locking primitive silently breaks mutual exclusion for any still-bash wrapper.
- **TIOCSTI detection has zero Rust logic today** — `grep -ri tiocsti src/` yields one config *comment*
  (`config.rs:1712`).
- The port is the moment to **collapse the three path encoders and two handle normalisers** (A17, A18), not
  after — otherwise it adds a fourth and a third.
- Every new file write, spawn, and network touch introduced by the port requires a STRIDE row in the same PR,
  against a table whose provenance column already points at deleted files (F3).

---

## Production-readiness gaps

**Shipping** — F1 in full; no `--version` flag anywhere (`grep '"--version"' src/` → empty; `CARGO_PKG_VERSION`
is used only in the TUI at `app/mod.rs:1460`, `render.rs:442`), which combined with `strip = true` means a
field binary cannot be identified; no shell completions; no man page; hand-rolled argument parsing
(`main.rs:38-117`, `cli.rs:26,76` with a bespoke `reject_unknown_flags`) rather than a parser that could
generate completions.

**Upgrade / downgrade** — no guard against opening a newer SQLite schema with an older binary (A20); config
migration exists and is well tested but there is no user-facing upgrade guide and no CHANGELOG.

**Error handling** — `anyhow` throughout with exactly **one** typed error enum in the codebase
(`src/lockfile.rs:52 AcquireError`). Callers cannot match on failure modes; user-facing messages depend on
string formatting. For a TUI whose tenet is "prefer explicit failure over silent waiting", typed errors at the
worker/UI boundary would be a real improvement.

**Observability** — the JSON-Lines `tracing` pipeline is good (daily rotation, 7 files, `RUST_LOG` override,
structured fields consumed by purge and doctor). Gaps: 50 legacy shim sites erase structure; five major
subsystems emit zero events; no correlation/run id ties a session's records across restarts; `dux doctor`
shells out to the bash `dux-amq-doctor` (`cli.rs:845,861`) so diagnostics break without the overlay; and the
doctor has no encryption section despite `SECURITY.md` promising one (O1). Crash log is unsanitized and
world-readable.

**Build hygiene** — the lib/bin double compilation (above).

**Repo hygiene** — missing `CHANGELOG.md`, `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `.editorconfig`,
pre-commit hooks, issue/PR templates. 49 unmerged remote branches. `dux-amq-rust/` and `probe-shots/` are empty
and **untracked** (`git log --all -- dux-amq-rust probe-shots` → nothing); `probe-shots/` is referenced nowhere
in the repo and is not in `.gitignore`.

**Docs** — `docs/` holds only `audits/`, `contributing/schema-policy.md`, and three `operations/` files. No
architecture overview, no runbook, no release process, no upgrade guide. Plus O13 and F3.

**Rust port status** — `docs/plans/rustport/artifacts/00-baseline.md` (untracked, dated today) is the only
rustport artefact; there is no phase 01+ plan, no scope, no acceptance criteria. Bash inventory it records:
doctor 850, `dux-amq/install.sh` 565, `claude-amq` 393, inject-bridge 307, `codex-amq` 242, `gemini-amq` 226,
finalize-migration 149, root `install.sh` ~150, plus verify/send/encoder/bashrc/secret-init — 3,627 total.

---

## Open questions

1. **Is the fork meant to ship a binary at all?** F1 is either the highest-severity defect in the repo or
   evidence that this is a personal deployment installed from source. The answer changes the priority of the
   entire release/supply-chain section. Nothing in the tree states the intent, and the documented install path
   currently 404s.
2. **The rustport contradicts standing audit guidance.** `docs/audits/audit03/06-architecture-modernity.md:61`
   states "the evidence does not support replacing … the shell wrappers"; `:46` calls wrapper extraction "not
   fixable"; `99-disposition.md:55` says these items "**must not be smuggled into the fix phase as unreviewed
   refactors**"; and `03-amq-transport-wrappers.md:57` says the wrapper layer "does not need a protocol
   framework." Grepping `PLAN.md`, `RESUME-PLAN.md`, and `PLAN-REVIEW-LOG.md` for any bash→Rust port language
   returns **zero** hits — the port is in no frozen spec. That may be a correct strategic reversal (audit03's
   remit was security and data loss, not strategy), but it should be written down and audit03's position
   formally superseded, or the next audit re-files it.
3. **What is `probe-shots/`?** Empty, untracked, referenced nowhere. The only adjacent evidence is
   `dux-amq/scripts/encode-claude-project-dir:11-14`, which describes deriving the encoder by *probing*
   Claude's on-disk naming. That is inference, not fact.
4. **Recover or abandon the audit01 lineage?** Seven NOT-DONE items exist as finished, tested commits on
   `origin/dux-amq-setup` (F2), against a diverged overlay layout. Port, re-implement, or formally drop — and
   either way delete the branch or document why it survives.
5. **Does live branch protection match `.github/BRANCH_PROTECTION.md`?** Needs `gh api` against the repo. The
   audit02 artifact says zero required checks were ever applied (F4). If that is still true, the CI "gates"
   are advisory and `CLAUDE.md:164` is wrong.
6. **What is the refresh policy for the 2026-07-11 pins?** Provider floors (`claude 2.1.163`, `codex 0.39.0`,
   `gemini 0.39.1`), `CLAUDE_PEERS_REV`, `SKILLS_PIN`/`SKILLS_REV`, `AMQ_TAG`, `DUX_TAG`, and the CI tool
   versions were frozen on one day with no cadence. The `deny.toml` ignores *had* a cadence and it silently
   lapsed behind a test that cannot detect lapsing (S5). The same will happen here — and the documented
   rotation procedure was deleted (F3).
7. **Is `opaline` (16k lifetime downloads, six months old, single maintainer) acceptable in a
   security-audited binary?** Vendor, replace, or accept explicitly in `SECURITY.md`.
8. **Do the 123 bats tests get retargeted at the Rust binary, or reimplemented as Rust tests?** They are the
   only written specification for TIOCSTI detection, the DUX2 envelope, install idempotency, the root
   installer, ownership records, and binary pinning. This decision should precede the first line of ported
   code.
9. **Does the decomposition get a render-snapshot safety net first?** `insta` + `TestBackend` golden files over
   the ~8 major layouts (agent pane, diff overlay, help, welcome, each modal), plus a shared `AppBuilder`
   fixture, before touching `render.rs`. Nothing else would catch a layout regression.
10. **Should `A1` (dead `SessionState` typestate) be finished or deleted?** `CLAUDE.md:87` directs new
    lifecycle code to use `transition()`, which no shipped code calls, and the enum has an undocumented sixth
    variant. Either wire it up during the decomposition or delete it and correct `CLAUDE.md` — leaving a
    documented-but-inert API through a large refactor is the worst of the three options.
