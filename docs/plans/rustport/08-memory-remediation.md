# Phase 08 — Memory remediation

**Track:** C (correctness & resources) · **Parallel-safe with:** 09, 10, 11
**Depends on:** nothing (coordinate item 9 with Phase 01) · **Blocks:** nothing

## Goal

Cut resident memory for a multi-agent host from ~50 MiB per pane to ~4 MiB, make the
existing safety valve correct and enabled, and close two genuinely unbounded buffers
reachable from child-process output.

## Evidence — measured, not estimated

Sizes were obtained by compiling a probe against the resolved
`alacritty_terminal 0.26.0` (`Cargo.lock:21-23`) and printing `size_of`; per-pane RSS
was measured by spawning real PTYs, saturating scrollback, and sampling process RSS.

```text
size_of::<Cell>()      = 24 bytes   (measured)
size_of::<Row<Cell>>() = 32 bytes   (measured, per-row overhead)

Row @  80 cols =  80 x 24 + 32 =  1,952 B
Row @ 200 cols = 200 x 24 + 32 =  4,832 B

10,050 rows @  80 cols = 18.7 MiB grid -> 20.4 MiB RSS measured
10,050 rows @ 200 cols = 46.3 MiB grid -> 50.7 MiB RSS measured
```

**16 live panes at 200 cols ≈ 816 MiB in grids alone.**

Three defaults conspire, all re-verified at the top level:

| Default | Value | Location |
|---|---|---|
| `agent_scrollback_lines` | **10,000** | `config.rs:820` |
| `default_max_panes()` | **0 = unlimited** | `config.rs:487` |
| `default_max_companion_terminals()` | **0 = unlimited** | `config.rs:495` |
| `default_enable_scrollback_overflow_autodetach()` | **false — the valve is off** | `config.rs:511` |
| `default_max_total_scrollback_mb()` | 256 | `config.rs:499` |
| `BYTES_PER_CELL` in the watchdog estimator | **4** vs real 24 + 32/row | `workers.rs:1202` |

The estimator therefore under-counts by **~6.1×**, so the 256 MiB cap actually fires
near **1.5–1.7 GiB** — and it is disabled by default anyway.

## Immediate mitigation (no code change, ship in docs now)

```toml
[ui]
agent_scrollback_lines = 2000
[limits]
max_panes = 8
enable_scrollback_overflow_autodetach = true
max_total_scrollback_mb = 42   # ~256 MiB real, compensating for the 6.1x estimator error
```

## In scope

Defaults, the estimator, unbounded buffers, the grid leak, syntect, and the diff/untracked-file
read paths.

## Out of scope

- Splitting `pty.rs` / `git.rs` / `diff.rs` → Phase 15.
- UI-thread blocking (a CPU/latency problem) → Phase 09.

## Work items

1. **Fix `BYTES_PER_CELL`** (`workers.rs:1202`): 4 → 24, and add the 32-byte per-row
   overhead the current arithmetic ignores entirely. Add a unit test asserting the
   estimate is within 15% of `size_of::<Cell>() * cols * rows + size_of::<Row>() * rows`,
   so a future `alacritty_terminal` bump cannot silently reintroduce the drift.
2. **Change the shipped defaults**: `agent_scrollback_lines` 10,000 → **2,000**,
   `max_panes` 0 → **8**, `enable_scrollback_overflow_autodetach` false → **true**.
   Update the config-file comments to state the real per-pane cost in MiB, since the
   config file is the documentation. Add a `migrate_config` arm so existing installs
   that still carry the old defaults are moved, using the `uses_legacy_defaults` guard
   pattern (arms 2→3, 3→4) so hand-customised values are left alone.
3. **Cap `vte`'s `osc_raw` growth (M7).** The upstream `is_full()` guards are
   `#[cfg(not(feature = "std"))]` and alacritty enables `std`, so an unterminated OSC
   from a child process grows without bound. Enforce a byte cap in
   `TerminalState::process` (`pty.rs`) and drop the sequence past it, logging once.
   **This is reachable from anything a child prints — treat as a resource-exhaustion
   defect, not a tidy-up.**
4. **Cap `App::raw_input_buf` (M8).** An unterminated OSC returns the entire buffer as
   remainder (`raw_input.rs:208-231`); demonstrated at **100,004 bytes**. Its sibling
   `loading_input_buf` is already capped at 64 via `append_capped`
   (`app/input.rs:52-65`, cap at `:19`). Use the same helper. Test with a
   100 KB unterminated OSC.
5. **Fix the PTY grid leak (M11).** When `PtyClient::drop` hits its timeout
   (`pty.rs:717-727`) the reader thread is detached and its entire 20–51 MiB grid plus
   2 fds leak, regardless of session state. **Close the reader fd before waiting** so
   the thread exits on EOF rather than being abandoned. Note the existing Drop test
   (`pty.rs:2075-2103`) asserts only that `recv_timeout(2s)` succeeds and is *designed
   to tolerate* the detach branch — so it passes either way. Write a test that fails on
   the leak (assert the thread joined, or that fds were released).
6. **Make syntect's `SyntaxCache` a `static OnceLock`.** It is currently rebuilt **per
   diff** (`workers.rs:3106`) — measured at 1.5 MiB and 0.8 ms to build, 3.7 MiB after
   highlighting a 2,236-line file — with no debounce and no in-flight guard. A ~3-line
   change makes it 1.5 MiB once for the process.
7. **Stop reading whole untracked files (M13).** `classify_untracked_file`
   (`git.rs:799`) reads **every untracked file in full, every 2 seconds**, when
   `content_inspector` only needs 1,024 bytes. Read a 1 KiB prefix. **Coordinate with
   Phase 01 item 9**, which replaces the abandoned `content_inspector` dependency —
   this is its only caller.
8. **Bound the diff path (M14, M15).** `diff.rs:50-75` holds ~4× file size live
   simultaneously; add a size cap with a clear "file too large to diff" message rather
   than an OOM. `similar::TextDiff::from_lines` runs Myers over the whole file with
   **no `.deadline()`** — set one.
9. **Bound the recovery scan (M20).** `resume_recovery.rs:675-704` reads **every byte
   of every transcript**, up to `MAX_TRANSCRIPT_FILES = 100,000` (`:20`). The count is
   bounded; the I/O is not. Filter by mtime window before parsing.
10. **Box the PTY handle inside `SessionState` (M17).** Every `Vec<AgentSession>`
    element is sized for the largest variant `Live { PtyHandle, .. }` (~150–200 B),
    including exited rows — roughly a 3× shrink.
11. **Add a per-file size cap to `dux.log` (M19).** Daily rotation with 7 files
    retained exists (`logger.rs:65-69`); there is no size bound within a day.
12. **Reconsider `mmap_size = 128 MiB` (`storage.rs:893`)** for the CLI path. It is
    virtual, but multiplied by every `dux peer` process as well as the app and the
    create-agent worker.
13. **Document the memory model** in `docs/operations/` — the per-pane arithmetic, what
    the knobs do, and the measurement recipe — so the next person does not have to
    re-derive it.

## Acceptance criteria

- [ ] `BYTES_PER_CELL` corrected, row overhead included, drift test present.
- [ ] New defaults shipped with a migration arm; config comments state real MiB costs.
- [ ] A 100 KB unterminated OSC from a child process does not grow `osc_raw` or
      `raw_input_buf` without bound — tested for both.
- [ ] A test fails if the PTY reader thread is abandoned on Drop timeout.
- [ ] `SyntaxCache` built once per process; a test or log assertion proves it.
- [ ] `classify_untracked_file` reads at most 1 KiB per file.
- [ ] Diff has a size cap and a `.deadline()`; oversized files produce a clear message.
- [ ] Recovery scan filters by mtime before reading transcript bodies.
- [ ] `SessionState`'s PTY handle boxed; `size_of::<AgentSession>()` measurably smaller.
- [ ] `dux.log` has a per-file size cap.
- [ ] **Measured**: 8 panes at 200 cols with saturated scrollback stays under 100 MiB RSS
      (was ~400 MiB at these settings, ~816 MiB at 16 panes).
- [ ] `docs/operations/memory-model.md` written.

## Validation

```bash
cargo test --all-features
# measure, do not assume:
/usr/bin/time -l cargo run --release   # macOS: peak footprint
#   open 8 agent panes at 200 cols, saturate scrollback, sample RSS
#   compare against the pre-change baseline recorded in the PR

cargo test --all-features pty
cargo test --all-features raw_input
```

## Risks

| Risk | Mitigation |
|---|---|
| Reducing default scrollback loses history users rely on | It is configurable and the migration arm only moves installs still on the old default; document the trade-off in the config comment |
| Capping OSC breaks a legitimate long sequence (e.g. OSC 52 clipboard) | `clipboard.rs:10` already caps OSC 52 at 100,000 bytes — align the new cap with it rather than choosing a smaller number |
| Closing the reader fd early truncates output on a clean exit | Only do it on the timeout path, after the graceful `recv_timeout` has already elapsed |
| `OnceLock` syntect changes theme reloading | The diff syntax theme is currently hardcoded (`diff.rs:77`, `"base16-ocean.dark"`) — Phase 10 makes it configurable; if it becomes runtime-settable, use `OnceLock` per theme or an `ArcSwap` |

## References

- `artifacts/research-runtime-memory.md` §2 (measurement method, derivation, table M1–M20)
- `artifacts/research-external-patterns.md` §Memory playbook
