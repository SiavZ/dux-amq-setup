# Audit 01 Implementation Plan

> Source: `dux-amq-audit.md` (2026-05-02). 24 findings (5 P0, 8 P1, 11 P2).
> Goal: bring the `dux-amq/` overlay + four Rust patches to production
> readiness without flipping Claude's default-on YOLO (intentional per user
> mandate; mitigated with documented threat model and Codex opt-out).

## How to use

Execute phases in numerical order. Each file is self-contained: read,
do the steps, run validation, tick acceptance. Pre-conditions are
explicit. Effort: **S** ≤ 1 h, **M** ≈ half-day, **L** ≈ full day+.

## Phase index

| #  | Title | Audit IDs | Effort |
|----|-------|-----------|--------|
| 00 | Preflight: verify assumptions, baseline, scaffolding | — | S |
| 01 | Supply-chain hardening (dux release, AMQ install, npx skill) | P0-2, P2-7 | L |
| 02 | Seeding default flip — opt-in, doc/code aligned | P0-3 | S |
| 03 | Finalize-migration safety — flock, atomic swap, drop `--delete` | P0-4 | M |
| 04 | Path encoding + realpath containment check | P0-5 | M |
| 05 | Codex `CODEX_AMQ_SAFE`, README "Security model", LUKS guidance | P0-1 (revised) | M |
| 06 | Upstream-sync automation; CODEOWNERS; `patches/` dir | P1-5 | M |
| 07 | TIOCSTI verification — confirm AMQ inject path | P1-1 | M |
| 08 | Wake-daemon startup probe + visible logs | P1-2 | S |
| 09 | Auto-resume concurrency cap + staleness skip + tests | P1-3, P2-3 | M |
| 10 | Scrollbar math + snapshot tests | P1-4, P2-3 | S |
| 11 | jq release lookup; jq as hard dep | P1-6 | S |
| 12 | Versioned `>>> dux-amq vN.M.K >>>` config inserts | P1-7 | S |
| 13 | AMQ binary pinning + hash-guarded shell-setup eval | P1-8 | M |
| 14 | P2 bundle: clipboard ST/Result, license, jq mode, bash nits | P2-1, 2, 4, 5, 8, 9, 10 | M |
| 15 | Overlay release pipeline — tarball + sha256 + cosign | P2-6 | M |
| 16 | `dux-amq-doctor` triage tool | P2-11 | M |
| 17 | Final validation — E2E smoke, kernel matrix, release gates | All | M |

## Known gaps / verification needed during execution

- **AMQ inject syscall** (Phase 07): whether `--inject-mode raw` uses
  `TIOCSTI` or PTY-master writes must be confirmed via `strace` on a live
  install before deciding to pin a kernel minimum or file an upstream patch.
- **Claude Code path encoder** (Phase 04): re-implemented from observed
  on-disk behavior; switch to `claude config sessions-dir` (or equivalent)
  if Anthropic ships one.
- **ratatui semantics** (Phase 10): `viewport_content_length` is documented
  upstream as imprecise (issue #1493); Phase 10 reads the pinned source.
- **AMQ signing** (Phases 13/15): we sha-pin; upgrade to cosign if AMQ
  ships signed releases.

## Out of scope

- Flipping Claude's `--dangerously-skip-permissions` default (per user mandate).
- Migration to Claude Code `auto mode` (future work, noted in Phase 05).
- Auditing AMQ internals or upstream dux beyond our four patched files.
