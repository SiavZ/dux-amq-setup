# Phase 21 — State encapsulation: finish what audit02 P1-V started

**Track:** E (serialized) · **Runs alone**
**Depends on:** 05, 16, 17 (the files must be split first) · **Blocks:** 24

## Goal

Complete the encapsulation audit02 specified and never delivered — so that 373 methods
no longer reach 117 fields, and the module boundaries created in Track D actually
constrain something.

## Evidence — the headline coherence defect

The plan at `docs/audits/audit02/audit02.md:174` specifies
`App { ui, runtime, git, remote, config, theme }` and, critically:

> *"Each submodule's `impl App` block becomes `impl GitState` / `impl UiState` etc. with
> thin `App` shims that delegate."*

Reality, verified:

- **`RemoteState` does not exist.**
- **There are zero `impl` blocks on any state sub-struct** — grep across `src/` confirms
  it. The only `fn` in `state/` is `RuntimeState::drop` (`state/runtime.rs:152-156`).
- All 72 sub-struct fields are `pub(crate)` inside `pub(crate)` structs, so
  `self.ui.focus` from `workers.rs` is **exactly as reachable** as `self.focus` was.
- The audit counted 120 `pub(crate)` fields; there are now **117** (45 loose on `App` +
  72 in sub-structs).

**Step 1 landed; step 2 never started.** It delivered indirection, not encapsulation —
and `input.rs` grew from 10,506 to 13,266 lines afterward.

## Why this is last among the refactors

Moving methods onto `impl UiState` / `impl GitState` / `impl RemoteState` before the
files are split means doing the work twice. Track D creates the module boundaries; this
phase makes them mean something.

## The unmeasured input

**No research agent measured which `App` fields each file actually mutates.** That
fan-out matrix is what determines whether this is a two-day or a two-week job.
**Work item 1 produces it before any code moves.**

## Work items

1. **Build the field-mutation fan-out matrix.** For each of the ~140 modules produced by
   Track D, record which of the 117 fields it reads and which it writes. Automate it —
   a `syn` pass over `self.<field>` / `self.<substruct>.<field>` expressions is
   sufficient and reusable. **Publish the matrix as an artifact**; it is the evidence
   base for every subsequent decision in this phase, and it answers open question #7 from
   the original research.
2. **Create `RemoteState`** and move the ~19 `RuntimeState` fields the audit earmarked
   for it: `gh_status`, the `pr_*` cluster, and the AMQ/orchestrator maps. This is the
   missing sub-struct, and its absence is why `RuntimeState` carries 33 fields.
3. **Confirm the Phase 16 field migration is complete** — no loose UI fields on `App`,
   `InputState` exists for the raw-input buffers, in-flight markers consolidated, and the
   two misplacements corrected (`last_pty_activity` out of `GitState`,
   `collapsed_projects` out of `GitState`).
4. **Move methods onto the sub-structs**, guided by the matrix. A method that touches
   exactly one sub-struct becomes an `impl <SubStruct>` method. A method that touches
   several stays on `App` and calls into them — that is the shim layer the audit
   specified.
5. **Narrow visibility, sub-struct by sub-struct.** The measurable goal: **fields become
   private to their sub-struct's module**, with accessors where genuinely needed. Do it
   one sub-struct at a time, starting with `UiState` (smallest blast radius) and finishing
   with `RuntimeState` (largest).
   **Track the count**: 117 `pub(crate)` fields today. Every one that becomes private is
   progress; the phase is not done while the number is still 117.
6. **Complete the `SessionState` typestate migration.** `CLAUDE.md` describes phase 2 —
   retiring the string `SessionStatus` and folding the PTY handle into the `Live` variant
   — as planned but not done. Verify how far it got and finish it. This overlaps with
   Phase 08 item 10 (boxing the handle); coordinate so the handle is boxed **and** the
   typestate lands together rather than being touched twice.
7. **Finish the `tracing` migration.** `CLAUDE.md` prefers structured
   `tracing::{info,warn,error,debug}!` with explicit `target: "dux::<module>"` over the
   legacy `crate::logger::*` shims. Count the remaining legacy call sites and convert
   them. Preserve the hard constraint documented at the top of `src/sanitize.rs`:
   **never call any logging shim from inside `crate::sanitize::*`** — it is on the legacy
   shim's call path and would recurse into a stack overflow.
8. **Update `CLAUDE.md:77` and `src/app/state/mod.rs:6-12`** to describe the new reality.
   Phase 04 corrected them to describe the *current* state; this phase changes that state,
   so they need correcting again. **Same commit.**
9. **Regenerate all module-tree headers.** Encapsulation changes the dependency graph
   substantially; `module-trees --check` will be red until they are regenerated.

## Acceptance criteria

- [ ] The field-mutation fan-out matrix exists as an artifact and is referenced by the PR.
- [ ] `RemoteState` exists and holds the ~19 earmarked fields; `RuntimeState` is
      correspondingly smaller.
- [ ] `impl UiState`, `impl GitState`, `impl RuntimeState`, `impl RemoteState` blocks
      exist and hold real methods (the count is > 0 — today it is exactly 0).
- [ ] **The `pub(crate)` field count is materially below 117**, with the final number and
      the remaining exceptions justified in the PR.
- [ ] Methods touching exactly one sub-struct live on that sub-struct.
- [ ] `SessionState` typestate phase 2 complete: string `SessionStatus` retired, PTY
      handle inside the `Live` variant (and boxed, per Phase 08).
- [ ] Zero remaining `crate::logger::*` call sites, or each survivor justified.
- [ ] `src/sanitize.rs` still calls no logging shim (grep-verified).
- [ ] `CLAUDE.md:77` and `state/mod.rs` describe the new layout.
- [ ] Phase 06 snapshots show zero diff.
- [ ] `cargo test --all-features` green, same test count.
- [ ] `module-trees --check` passes after regeneration.

## Validation

```bash
cargo test --all-features
cargo insta test                    # zero snapshot movement — this is a pure refactor

# the measurable outcome
grep -rn 'pub(crate)' src/app/state/ | wc -l      # must be well below the 117 baseline
grep -rc 'impl UiState\|impl GitState\|impl RuntimeState\|impl RemoteState' src/ | grep -v ':0'

# the sanitizer recursion guard
grep -n 'tracing::\|logger::' src/sanitize.rs     # expect: no output

# legacy logging
grep -rn 'crate::logger::' src/ | wc -l           # expect: 0

ci/check-file-length.sh
cargo modules dependencies --acyclic --bin dux
cargo run -p xtask -- module-trees --check
```

## Risks

| Risk | Mitigation |
|---|---|
| The fan-out is so dense that encapsulation is impractical | That is exactly what work item 1 measures. If a sub-struct proves impossible to close, **say so with the matrix as evidence** and narrow the goal deliberately — do not silently declare victory at 117 fields again, which is how audit02 P1-V ended |
| Narrowing visibility cascades into hundreds of accessors | Accessor sprawl is a failure mode too. Prefer moving the *method* to the data over adding a getter; a sub-struct with 20 getters has not been encapsulated |
| `SessionState` typestate change touches persistence | `tests/storage_migrations.rs` and `tests/session_state.rs` cover it; the string status is persisted on every row, so retiring it needs a schema migration — treat as a real migration, not a refactor |
| Logging conversion changes log output that tooling parses | GDPR purge, the doctor, and log-grep aliases consume these records. Structured fields are the goal; verify `purge.rs`'s log redaction still matches |
| Snapshot churn suggests behaviour changed | In a pure refactor it is a defect. Investigate; do not accept the new snapshot |

## References

- `artifacts/research-app-monoliths.md` §C1 (the headline defect), §C2, §Open question 1 and 7
- `artifacts/research-runtime-memory.md` §5.5 (incomplete migrations)
- `CLAUDE.md` §Session lifecycle, §Logging
