# dux rustport — full production-readiness plan

> **This is not an MVP plan.** Every phase below must land before the stack is
> considered production ready. Nothing is deferred to a "later" bucket. Where a
> phase names something as out of scope, that item belongs to a *different phase
> in this same set*, never to an unplanned future.

## What this actually is

The premise "port dux to Rust" needed correcting before planning began. **dux is
already Rust** — 69,515 lines across `src/`. The port surface is ~3,600 lines of
bash under `dux-amq/`. The larger body of work your brief describes — modularity,
the 500-line rule, coherence, "upgrade debt we've been avoiding" — is a
**refactor of the existing Rust**, plus a **bash→Rust port** of the overlay.

Both are in scope. They are different tracks with different risk profiles.

## Research basis

Every phase cites verified evidence. Six parallel research agents, each fanning
out to its own sub-researchers, produced the briefs in `artifacts/`. Claims were
required to cite `file:line`; unverifiable claims came back as open questions
rather than guesses. Several agent claims were re-verified at the top level and
**two were corrected** — see `artifacts/research-app-monoliths.md` §Verification
notes.

| Artifact | Domain | Lines |
|---|---|---|
| `artifacts/00-baseline.md` | Build health, size census, toolchain drift | 85 |
| `artifacts/research-app-monoliths.md` | `src/app/` structure, seams, coherence defects | 444 |
| `artifacts/research-config-keys-theme-cli.md` | Config schema, keybindings, theme, CLI, provider tenet | 1,205 |
| `artifacts/research-runtime-memory.md` | PTY/storage/git/watch/peer + **measured** memory | 662 |
| `artifacts/research-bash-port.md` | Behavioral specs for all 11 shell entry points | 1,278 |
| `artifacts/research-debt-ci-security.md` | Deferred-work ledger, CI, supply chain, security | 741 |
| `artifacts/research-external-patterns.md` | Architecture precedent, memory playbook, tooling | 1,029 |
| `artifacts/bats-test-parity-matrix.md` | All 123 bats tests = port acceptance criteria | — |

## The five findings that shaped this plan

1. **The fork has never shipped its own code.** All four `v*` tags are upstream
   ancestors; `v0.4.0`'s `src/` contains none of the ~11,469 LOC of fork-only
   modules. `api.github.com/repos/SiavZ/dux-amq-setup/releases/latest` returns
   **HTTP 404** (one release exists, marked prerelease), so the documented install
   path fails at `install.sh:97` today. Every Rust-side STRIDE mitigation lives in
   code no released artifact contains. → **Phase 03**

2. **The memory problem is PTY grids, and it was measured.** 24 B/cell + 32 B/row
   against a 10,000-line default scrollback = **20.4 MiB per pane at 80 cols,
   50.7 MiB at 200** (real RSS). `max_panes` defaults to unlimited, the overflow
   watchdog defaults to **off**, and its estimator uses `BYTES_PER_CELL = 4` — a
   **~6.1× under-count**, so a 256 MiB cap actually fires near 1.6 GiB.
   → **Phase 08**

3. **The last decomposition half-landed, and the file grew afterward.** audit02
   P1-V specified `impl UiState`/`impl GitState` with delegating shims. There are
   **zero** such blocks, `RemoteState` was never created, 45 fields remain loose on
   `App`, and `input.rs` went from 10,506 → 13,266 lines *after* being flagged.
   Field count moved 120 → 117. That is indirection, not encapsulation.
   → **Phases 16, 17, 21**

4. **There is no safety net for the riskiest refactor.** `render.rs` has 43 tests
   and every one covers a pure helper — zero call any `render_*` method, and it
   uses `TestBackend` zero times. `render_prompt`'s 25 arms each write
   `ui.overlay_layout.active`, the sole contract with mouse hit-testing. Drop one
   during extraction and mouse support dies silently. → **Phase 06 gates Phase 17**

5. **"Any CLI tool can be a provider" is false.** 12 production sites branch on
   provider name. `purge.rs:132-134` hardcodes state directories for four
   providers, so **a user-added provider's chat history is never purged** — a
   completeness gap in a compliance feature. → **Phases 11, 22**

## Execution order

Phases are numbered in dependency order. **Within a track, phases are
parallel-safe with each other.** Tracks A–D fan out; Track E must be serialized.

```text
TRACK A — Foundation & gates            [4 phases, fully parallel, start immediately]
├── 01 toolchain-and-dependencies
├── 02 ci-gates-and-modularity-enforcement
├── 03 release-pipeline-and-supply-chain
└── 04 documentation-integrity

TRACK B — Safety nets                   [3 phases, parallel; GATE for Track D]
├── 05 test-fixture-unification
├── 06 render-characterization-harness
└── 07 config-property-tests

TRACK C — Correctness & resources       [4 phases, parallel; independent of D]
├── 08 memory-remediation
├── 09 ui-thread-blocking
├── 10 live-defect-fixes
└── 11 provider-capabilities

TRACK D — Decomposition                 [6 phases, parallel; requires Track B]
├── 12 config-decomposition
├── 13 keys-decomposition
├── 14 cli-theme-decomposition
├── 15 runtime-subsystem-decomposition
├── 16 app-core-decomposition
└── 17 app-input-render-decomposition

TRACK E — Serialized integration        [7 phases, strictly sequential]
├── 18 workspace-and-dux-amq-rust-foundation
├── 19 dux-amq-rust-wrappers-and-bridge
├── 20 dux-amq-rust-doctor-and-installer
├── 21 state-encapsulation
├── 22 session-capture-strategy
├── 23 threat-model-and-security-docs
└── 24 final-validation-and-release
```

### Dependency graph (the edges that actually constrain)

```text
01 ──────────────────────────────────────────────► 18   (new crate must be linted by the new toolchain)
02 ──────────────────────────────────────────────► 12..17 (the 500-line gate must exist before splits claim compliance)
05 ──► 16, 17, 21                                        (one App::for_test before any field moves)
06 ──► 17                                                (golden buffers before render_prompt is touched)
07 ──► 12                                                (template-coverage property test before config_schema is split)
11 ──► 22                                                (capabilities land before the capture strategy is extracted)
12..17 ─────────────────────────────────────────► 21     (encapsulation is the last step, not the first)
18 ──► 19 ──► 20                                         (crate, then applets, then installer delegation)
19, 20 ─────────────────────────────────────────► 23     (threat model updates once the new surface exists)
all ────────────────────────────────────────────► 24
```

### Why this ordering

- **01 before everything that writes new code.** The toolchain is pinned at
  1.88.0 (2025-06-23) while stable is 1.98.0 (2026-08-18). Every clippy lint added
  across ten releases is currently invisible. Writing 42 new modules and ~140 split
  files against a stale lint set guarantees a second, larger cleanup later.
- **Track B before Track D, without exception.** 1,714 test lines in `config.rs`,
  760 in `keybindings.rs`, 785 in `cli.rs` reach private items via `use super::*`.
  Splitting the modules breaks these en masse. The test migration is its own commit,
  landed first.
- **Track C is deliberately independent.** These are user-visible defects and
  resource bugs. They must not wait behind a multi-week refactor, and none of them
  requires the decomposition to have happened.
- **21 is last among the refactors, not first.** Finishing audit02 P1-V means
  moving methods onto `impl UiState`/`impl GitState`/`impl RemoteState`. Doing that
  before the files are split means doing it twice.

## Conventions

Every phase is bound by `CONVENTIONS.md` — the file-header format (including the
`` ```text `` fence rule that keeps module trees from being compiled as doctests),
the 500-line rule and its opt-out, test placement, and commit discipline. Read it
before starting any phase.

## Phase file structure

Each phase file carries: Goal · Evidence · In/Out of scope · numbered Work items
with `file:line` · Module tree where applicable · Acceptance criteria as
checkboxes · Validation commands · Risks · References.

**A phase is done when every acceptance checkbox is ticked and every validation
command passes.** Not before.

## Baseline to preserve

`cargo fmt --all -- --check` and
`cargo clippy --all-targets --all-features -- -D warnings` both exit **0** today,
and `cargo test` is green with **1,139 distinct tests** (1,056 unit + 83
integration; the raw 1,149 double-counts the lib suite under the bin target). The
123 bats tests run in a separate CI job (`.github/workflows/overlay-ci.yml:38`).

**Any warning or failure introduced by this work is a regression, not
pre-existing noise.**
