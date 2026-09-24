# Finding disposition ledger

Baseline: `3d5207486cfd3e75b96fc3ea550430f0439da998`  
Audit-phase initialization: 2026-07-11  
Counts: **9 P0, 28 P1, 2 P2; 39 confirmed**

This ledger is initialized exactly as required for the audit phase: every finding is in state **`confirmed`**. “Fixable” means the item survived the severity/refutation threshold and has a bounded proof direction; it does not mean a fix was made. The later fix phase must update state and attach actual test/gate evidence without deleting or renumbering rows.

States: `confirmed` (open), `partial (SHA)` (gate-unblocking slice landed, remainder open), `fixed (SHA)` (closure proof landed in that commit), `refuted (reason)`, `deferred (reason)`. P0 tier fixed on branch audit03/p0, 2026-07-11; threat-model updates for P0-03/P0-04/P0-09 in 3848c3d.

| ID | Tier | State | Fixable | Finding | Minimum closure proof |
|---|:---:|:---:|:---:|---|---|
| P0-01 | P0 | fixed (3bba098) | yes | Installer reruns overwrite the user's Dux configuration | rerun fixture preserves comments, unknown keys, projects, macros, args, and custom settings |
| P0-02 | P0 | fixed (8aecbfe) | yes | Stale-inflight recovery can overwrite a newer queued message | collision test preserves both bodies using no-replace/quarantine semantics |
| P0-03 | P0 | fixed (be7bf48) | yes | Purge deletes the only session identity after an earlier cleanup failure | injected category failures retain durable row/plan and permit successful retry |
| P0-04 | P0 | fixed (be7bf48) | yes | Purge does not enforce its promised deletion containment | traversal/absolute/symlink/root tests refuse; valid descendants still purge |
| P0-05 | P0 | fixed (18a1353) | yes | Updating `.git/info/exclude` converts a read failure into destructive overwrite | invalid-UTF8/read-error fixtures preserve original bytes and fail closed |
| P0-06 | P0 | fixed (a387d00) | yes | PTY teardown can block forever when a descendant keeps the slave open | background-descendant teardown completes within a hard deadline |
| P0-07 | P0 | fixed (e8cfe85) | yes | A finite watch capture can panic reset processing | huge finite/boundary values return fallback, never unwind, for every unit |
| P0-08 | P0 | fixed (bebeb8a) | yes | Unicode macro text can panic the normal render path | emoji/CJK/combining macro rendering at boundary widths does not panic or split UTF-8 |
| P0-09 | P0 | fixed (07f43dd) | yes | The Codex wrapper disables hook trust review by default | argv tests show bypass absent by default and present only after explicit opt-in |
| P1-01 | P1 | fixed (6fb7b2b) | yes | Custom `STATE_ROOT` installs are split across custom and hard-coded roots | custom-root end-to-end fixture contains no operational `/data/state` fallback |
| P1-02 | P1 | fixed (6fb7b2b) | yes | Wrapper identity claiming has a check-then-create race | synchronized concurrent-start test yields exactly one owner/launcher |
| P1-03 | P1 | fixed (6fb7b2b) | yes | `_unrouted` is both a valid handle and a reserved routing sentinel | legitimate sentinel-like handle routes to itself; fallback remains disjoint |
| P1-04 | P1 | fixed (6fb7b2b) | yes | Strict HMAC mode does not provide a coherent authenticated-delivery boundary | multiline, wrong-recipient, simultaneous replay, and mixed-queue tests authenticate exact delivered bytes |
| P1-05 | P1 | fixed (830bc31 + 4b27a74) | yes | Doctor's hash, JSON, anonymization, and read-only contracts are broken | Linux hash match; one JSON value; full redaction; database bytes/schema unchanged |
| P1-06 | P1 | fixed (605ca48) | yes | CI shell lint omits the highest-risk extensionless overlay scripts | CI/local lint enumerate every executable shell script and stay in parity |
| P1-07 | P1 | fixed (605ca48) | yes | The fork's release installer downloads a different upstream binary | release fixture resolves/downloads archive from the releasing repository |
| P1-08 | P1 | fixed (605ca48) | yes | The root installer ignores the release checksum and attestation | tampered archive is rejected before extraction using published evidence |
| P1-09 | P1 | fixed (605ca48) | yes | Release packaging feeds an ISO timestamp to GNU tar's epoch syntax | representative release timestamp packages successfully and reruns hash-identically |
| P1-10 | P1 | fixed (605ca48) | yes | Release and security-gate tools are selected from a moving latest version | every cargo-installed CI tool has an explicit reviewed version |
| P1-11 | P1 | fixed (605ca48) | yes | Claude Peers is installed and updated from an unpinned default branch | installer checks out an explicit commit/tag and verifies recorded identity |
| P1-12 | P1 | fixed (6fb7b2b) | yes | Provider wrappers enforce no minimum safe CLI version | each wrapper accepts supported current version and rejects/warns below reviewed floor |
| P1-13 | P1 | fixed (9332e88 + 605ca48) | yes | Two newly open RustSec advisories make the exact-lock security gate fail | lock/gate resolves or time-boundedly justifies both IDs; exact advisory query is clean/expected |
| P1-14 | P1 | fixed (773a6b0) | yes | Configured periodic backups are never started | App wiring test observes scheduled Online Backup API output and zero disables it |
| P1-15 | P1 | fixed (867b435) | yes | Schema migrations are not atomic despite documentation saying they are | injected mid-migration failure rolls back DDL and version; retry succeeds |
| P1-16 | P1 | fixed (773a6b0) | yes | Session settings mutate live memory before persistence succeeds | forced upsert failure preserves old memory/runtime/database while draft remains retryable |
| P1-17 | P1 | fixed (867b435) | yes | AMQ scanning caps an arbitrary directory subset before sorting | over-cap multi-receiver test delivers deterministic oldest/fair selection |
| P1-18 | P1 | fixed (867b435) | yes | Busy and missing-PTY deliveries never enter the timeout-warning state | fresh matched busy/no-PTY receiver warns once after configured duration |
| P1-19 | P1 | fixed (867b435) | yes | The documented `u64::MAX` quiet-window escape hatch holds forever | max-value test matches corrected documented semantics |
| P1-20 | P1 | fixed (9c4c694) | yes | Generated and deserialized UI defaults disagree | absent/partial UI and `Config::default` produce identical pane defaults |
| P1-21 | P1 | fixed (24deeee) | yes | Keybinding conflict validation disagrees with runtime matching | modifier-subset pairs rejected exactly when runtime lookup overlaps |
| P1-22 | P1 | fixed (24deeee) | yes | Diff generation turns read errors into invented empty files | Git/worktree read failures surface errors; true new/deleted/binary cases remain correct |
| P1-23 | P1 | fixed (773a6b0) | yes | The main UI path still performs blocking filesystem and Git work | listed handlers enqueue workers; delayed fake I/O leaves input loop responsive |
| P1-24 | P1 | fixed (24deeee) | yes | Changed-file polling can fork once per untracked file every two seconds | process-count/perf fixture stays bounded as untracked count grows |
| P1-25 | P1 | fixed (4b27a74) | yes | Purge derives AMQ and log targets differently from the runtime | basename/branch normalization and absolute-log fixtures purge exact runtime targets |
| P1-26 | P1 | fixed (773a6b0) | yes | Config diff omits behavior-changing sections and arguments | one-field fixtures for every typed section appear; no-change alone claims defaults |
| P1-27 | P1 | fixed (773a6b0) | yes | Lifecycle persistence failures can orphan worktrees and report false success | injected DB errors compensate or retain/adopt worktree and never emit success |
| P1-28 | P1 | fixed (9c4c694) | yes | Sole configuration writes lack an atomic replacement rail | interrupted/failed replacement leaves prior valid config intact |
| P2-01 | P2 | fixed (b13df41) | yes | Terminal widths use Unicode scalar counts despite a width-aware renderer | mechanical ratatui-width replacement passes CJK/emoji/combining tests |
| P2-02 | P2 | fixed (b13df41) | yes | A private fake re-export and no-op function are safely deletable | delete alias/function/import; normal compile/test confirms no consumer |

## Non-finding observations

The large modules, wrapper duplication, historical phase comments, and broad dependency-modernity questions in [06-architecture-modernity.md](06-architecture-modernity.md) are intentionally absent from this ledger. They did not meet P2's safe-deletion/mechanical/test-proof threshold and must not be smuggled into the fix phase as unreviewed refactors.

## Review-round follow-ups

- **N5 (deferred, perf):** per-diff `SyntaxCache::new()` in `dispatch_diff` loses cross-diff syntax-cache reuse after the P1-23 thread-safety change. Marked with a `// ponytail:` comment at `src/app/workers.rs`; upgrade path is a worker-owned `Arc<Mutex<SyntaxCache>>` if diff latency ever matters. Not a correctness issue.
